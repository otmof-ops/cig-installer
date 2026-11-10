// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `mirror <dir>` (#16): the channel's manifest and every build of the chosen cartons, each
//! admitted, in a directory laid out the way `install --from <dir>` and `CIGI_MIRROR` read it.
//!
//! Safe to run again and again: a file already there that is still admissible is left as it is,
//! the rest is fetched, every file lands through a partial file and a rename, builds go in before
//! the manifest that names them, and nothing is ever removed. Clients treat a mirror as one
//! source (no second place agrees with it), whatever put the files there.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cig_compat::{Manifest, Target, Version};
use cigi_source::fetch::{artifact_locations, manifest_cache_dir};
use cigi_source::{get_artifact_for, sweep_downloads, DigestTrust, NoAttest, Trust, TrustLevel};
use cigi_ui::{InstallerError, Report, EXIT_REFUSED};
use serde_json::json;

use super::bundle::{builds_for, parse_targets, plain};
use super::health::size_text;
use super::install::{pick_carton, Bar, InstallArgs};
use crate::cx::{Cx, ManifestMode};
use crate::errors::{bug, ie};

#[derive(Debug, Clone)]
pub struct MirrorArgs {
    pub dir: PathBuf,
    /// An exact carton or a pin; with none, every carton of the channel that is not yanked.
    pub carton: Option<String>,
    /// With none, every target the chosen builds are published for.
    pub targets: Vec<String>,
}

fn refused(msg: impl Into<String>) -> InstallerError {
    InstallerError::new("E1838", msg)
        .with_fix("ciginstaller mirror ./mirror")
        .with_exit(EXIT_REFUSED)
}

/// Write `bytes` or the file `from` to `dest` through a partial file, world-readable so a web
/// server running as someone else can serve it.
fn put(dest: &Path, bytes: Option<&[u8]>, from: Option<&Path>) -> Result<(), InstallerError> {
    let name = dest
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dest.with_file_name(format!(
        ".{name}.partial-{}",
        cigi_source::digest::random_suffix()
    ));
    let r = (|| -> std::io::Result<()> {
        let mut out = cigi_source::http::create_private(&tmp)?;
        match (bytes, from) {
            (Some(b), _) => out.write_all(b)?,
            (None, Some(p)) => {
                let mut src = std::fs::File::open(p)?;
                let mut buf = vec![0u8; 64 * 1024];
                loop {
                    let n = src.read(&mut buf)?;
                    if n == 0 {
                        break;
                    }
                    out.write_all(&buf[..n])?;
                }
            }
            (None, None) => {}
        }
        out.sync_all()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644))?;
        }
        std::fs::rename(&tmp, dest)
    })();
    if let Err(e) = r {
        let _ = std::fs::remove_file(&tmp);
        let code = if e.raw_os_error() == Some(28) {
            "E1836"
        } else {
            "E1838"
        };
        return Err(
            InstallerError::new(code, format!("cannot write {}: {e}", dest.display()))
                .with_fix(if code == "E1836" {
                    "ciginstaller gc --dry-run"
                } else {
                    "ciginstaller mirror ./mirror"
                })
                .with_exit(EXIT_REFUSED),
        );
    }
    Ok(())
}

/// Is this file already in place and still admissible, without a request? The check is the
/// installer's own trust (the floor included), with the attestation step left out because that
/// one needs the network: a floor that needs it simply fetches again.
fn in_place(cx: &Cx, art: &cig_compat::Artifact, asset: &Path, side: &Path) -> bool {
    if !asset.is_file() {
        return false;
    }
    let side_text = std::fs::read_to_string(side).ok();
    let offline_trust = DigestTrust::new(cx.trust.floor)
        .with_attest(Arc::new(NoAttest))
        .with_single_source(true);
    let Ok(v) = offline_trust.admit_artifact(art, side_text.as_deref(), asset) else {
        return false;
    };
    // the rule `get_artifact_for` applies to a source that is not independent
    if !cx.source.independent()
        && v.level == TrustLevel::Checksummed
        && !cx.trust.allows_single_source()
    {
        return false;
    }
    true
}

pub fn run(cx: &mut Cx, a: &MirrorArgs) -> Result<Report, InstallerError> {
    if cx.globals.offline {
        return Err(refused(
            "a mirror is filled from downloads, and --offline allows no request",
        ));
    }
    let targets_asked = if a.targets.is_empty() {
        Vec::new()
    } else {
        parse_targets(&a.targets)?
    };
    cx.lock_and_recover()?;
    std::fs::create_dir_all(&a.dir)
        .map_err(|e| refused(format!("cannot use {} as a mirror: {e}", a.dir.display())))?;
    let dir = std::fs::canonicalize(&a.dir)
        .map_err(|e| refused(format!("cannot use {} as a mirror: {e}", a.dir.display())))?;
    if !dir.is_dir() {
        return Err(refused(format!("{} is not a directory", dir.display())));
    }
    // compare canonical forms: a source or a home named through a link (macOS: /var is
    // /private/var) is the same directory as `dir`, which was canonicalized above
    let canon = |p: &std::path::Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let reads_from = cx
        .source
        .name()
        .strip_prefix("dir:")
        .map(|s| canon(std::path::Path::new(s)));
    if reads_from.as_deref() == Some(dir.as_path()) {
        return Err(refused(format!(
            "{} is the mirror this run reads from; give it another directory",
            dir.display()
        )));
    }
    if dir.starts_with(canon(&cx.home.root())) {
        return Err(refused(format!(
            "{} is inside the home {}; a mirror lives outside it",
            dir.display(),
            cx.home.root().display()
        )));
    }

    let m: Manifest = cx.manifest(ManifestMode::Fresh)?.clone();
    let channel = m.channel.clone();
    // a mirror that went backwards would hand clients a manifest they refuse
    let manifest_name = format!("manifest-{channel}.toml");
    if let Ok(text) = std::fs::read_to_string(dir.join(&manifest_name)) {
        if let Ok(old) = Manifest::from_str(&text) {
            if old.sequence > m.sequence {
                return Err(refused(format!(
                    "{} already holds the {channel} manifest at sequence {}, newer than the \
                     {} this run fetched",
                    dir.display(),
                    old.sequence,
                    m.sequence
                )));
            }
        }
    }

    // which builds
    let mut cartons: Vec<String> = Vec::new();
    let mut skipped_yanked: Vec<String> = Vec::new();
    let mut tools: BTreeSet<(String, Version)> = BTreeSet::new();
    let chosen: Vec<cig_compat::Carton> = match &a.carton {
        Some(_) => vec![pick_carton(cx, &m, &InstallArgs::carton(a.carton.clone()))?.clone()],
        None => m.cartons.values().cloned().collect(),
    };
    for c in &chosen {
        if m.is_carton_yanked(&c.version) {
            skipped_yanked.push(c.version.to_string());
            continue;
        }
        cartons.push(c.version.to_string());
        for (t, v) in &c.products {
            tools.insert((t.clone(), v.clone()));
        }
    }
    if cartons.is_empty() {
        return Err(InstallerError::new(
            "E1811",
            format!("the {channel} channel has no carton to mirror"),
        )
        .with_fix("ciginstaller carton list")
        .with_exit(EXIT_REFUSED));
    }
    let targets: Vec<Target> = if targets_asked.is_empty() {
        let mut set: BTreeSet<Target> = BTreeSet::new();
        for (t, v) in &tools {
            if let Some(b) = m.builds.get(&(t.clone(), v.clone())) {
                set.extend(b.artifacts.keys().filter_map(|k| Target::parse(k)));
            }
        }
        if set.is_empty() {
            set.insert(cx.target.clone());
        }
        set.into_iter().collect()
    } else {
        targets_asked
    };
    let tool_list: Vec<(String, Version)> = tools.into_iter().collect();
    let builds = builds_for(&m, &tool_list, &targets);

    // a file name two different builds would share cannot be served
    let mut names: BTreeMap<String, String> = BTreeMap::new();
    for (art, _) in &builds.arts {
        let (al, sl) = artifact_locations(art);
        for n in [al.file_name(), sl.file_name()] {
            if !plain(&n) {
                return Err(refused(format!("{n} is not a file name a mirror can hold")));
            }
        }
        if let Some(old) = names.insert(al.file_name(), art.sha256.clone()) {
            if old != art.sha256 {
                return Err(refused(format!(
                    "two builds are both published as {}",
                    al.file_name()
                )));
            }
        }
    }

    let _ = sweep_downloads(&cx.home);
    let mut added: Vec<String> = Vec::new();
    let mut kept = 0usize;
    let mut bytes_added = 0u64;
    let mut done_assets: BTreeSet<String> = BTreeSet::new();
    for (art, target) in &builds.arts {
        let (al, sl) = artifact_locations(art);
        let (asset_name, side_name) = (al.file_name(), sl.file_name());
        if !done_assets.insert(asset_name.clone()) {
            continue;
        }
        let (asset_path, side_path) = (dir.join(&asset_name), dir.join(&side_name));
        if in_place(cx, art, &asset_path, &side_path) {
            kept += 1;
            continue;
        }
        let mut bar = Bar(cx.ui.progress(
            &format!("fetching {} {}", art.tool, art.version),
            Some(art.size),
        ));
        let staged = get_artifact_for(
            cx.source.as_ref(),
            &cx.trust,
            &m,
            &art.tool,
            &art.version,
            target,
            &cx.home,
            &mut bar,
        )
        .map_err(ie)?;
        let r = put(&asset_path, None, Some(&staged.path)).and_then(|()| match &staged.sidecar {
            Some(text) => put(&side_path, Some(text.as_bytes()), None),
            None => Ok(()),
        });
        let _ = std::fs::remove_file(&staged.path);
        r?;
        bytes_added += art.size;
        added.push(asset_name);
    }

    // the manifest goes in last: nothing it names is missing when a client first sees it
    let cache = manifest_cache_dir(&cx.home, &channel);
    let doc = std::fs::read(cache.join("manifest.toml"))
        .map_err(|e| bug(format!("the admitted manifest is not in the cache: {e}")))?;
    let side = std::fs::read(cache.join("manifest.toml.sha256")).map_err(|e| {
        bug(format!(
            "the admitted manifest's checksum is not in the cache: {e}"
        ))
    })?;
    let mut manifest_written = false;
    for (name, body) in [
        (manifest_name.clone(), doc),
        (format!("{manifest_name}.sha256"), side),
    ] {
        let dest = dir.join(&name);
        if std::fs::read(&dest).is_ok_and(|old| old == body) {
            continue;
        }
        put(&dest, Some(&body), None)?;
        manifest_written = true;
    }

    let head = if added.is_empty() && !manifest_written {
        format!(
            "{} is already up to date with the {channel} channel",
            dir.display()
        )
    } else {
        format!(
            "{} now holds the {channel} channel, sequence {}",
            dir.display(),
            m.sequence
        )
    };
    let mut r = Report::done(head).detail(format!(
        "{} builds for {}: {} added ({}), {} already there",
        builds.arts.len(),
        targets
            .iter()
            .map(|t| t.to_string())
            .collect::<Vec<_>>()
            .join(", "),
        added.len(),
        size_text(bytes_added),
        kept
    ));
    r = r.detail(format!("cartons: {}", cartons.join(", ")));
    if !skipped_yanked.is_empty() {
        r = r.detail(format!("left out, yanked: {}", skipped_yanked.join(", ")));
    }
    if !builds.missing.is_empty() {
        r = r.detail(format!(
            "{} build and target pairs have no published file and were skipped",
            builds.missing.len()
        ));
    }
    r = r.detail(
        "a mirror is one source and unsigned until #9: clients reach pinned for builds they \
         have pins for, and otherwise need allow_single_source",
    );
    Ok(r.next(format!(
        "ciginstaller install carton --from {}",
        dir.display()
    ))
    .data(json!({
        "dir": dir.display().to_string(),
        "channel": channel,
        "sequence": m.sequence,
        "cartons": cartons,
        "targets": targets.iter().map(|t| t.to_string()).collect::<Vec<_>>(),
        "added": added,
        "kept": kept,
        "bytes_added": bytes_added,
        "manifest_written": manifest_written,
        "left_out_yanked": skipped_yanked,
    })))
}
