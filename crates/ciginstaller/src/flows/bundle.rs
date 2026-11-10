// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `bundle` (#16): one file holding a carton's manifest and the builds of chosen targets, for a
//! machine with no network. Built only from artifacts this run admitted at the usual levels (the
//! store cannot be a source: it holds unpacked files, and the manifest's digests name the
//! archives). The bytes are checked again as they are written. Unsigned until #9.

use std::path::{Path, PathBuf};

use cig_compat::{Artifact, Manifest, Target, Version};
use cigi_source::fetch::{artifact_locations, manifest_cache_dir};
use cigi_source::{
    get_artifact_for, sweep_downloads, write_bundle, BundleData, BundleError, BundleFile,
};
use cigi_ui::{InstallerError, Report, EXIT_REFUSED, EXIT_USAGE};
use serde_json::json;

use super::health::size_text;
use super::install::{pick_carton, Bar, InstallArgs};
use crate::cx::{Cx, ManifestMode};
use crate::errors::{bug, ie};

#[derive(Debug, Clone)]
pub struct BundleArgs {
    /// An exact carton or a pin (default: the newest that is not yanked).
    pub carton: Option<String>,
    /// `linux-x86_64`, ... (default: this machine's).
    pub targets: Vec<String>,
    pub out: PathBuf,
}

fn usage(msg: impl Into<String>, fix: &str) -> InstallerError {
    InstallerError::new("E1870", msg)
        .with_fix(fix)
        .with_exit(EXIT_USAGE)
}

/// `--targets a,b` as targets; each must look like `os-arch`.
pub(super) fn parse_targets(list: &[String]) -> Result<Vec<Target>, InstallerError> {
    let mut out: Vec<Target> = Vec::new();
    for t in list {
        let parsed = Target::parse(t.trim()).ok_or_else(|| {
            usage(
                format!("`{t}` is not a target; a target looks like linux-x86_64"),
                "ciginstaller bundle --targets linux-x86_64 --out carton.tar",
            )
        })?;
        if !out.contains(&parsed) {
            out.push(parsed);
        }
    }
    Ok(out)
}

/// A file name the layout can hold: the same test the mirror source applies when it reads.
pub(super) fn plain(name: &str) -> bool {
    cigi_source::location::is_plain_segment(name)
}

/// Every build of `tools` that `targets` need, once each (an `any` archive is one build for
/// all). `missing` collects `(tool, version, target)` for a target a build has no file for.
pub(super) struct Builds {
    pub arts: Vec<(Artifact, Target)>,
    pub missing: Vec<(String, Version, Target)>,
}

pub(super) fn builds_for(m: &Manifest, tools: &[(String, Version)], targets: &[Target]) -> Builds {
    let mut b = Builds {
        arts: Vec::new(),
        missing: Vec::new(),
    };
    for (tool, v) in tools {
        for t in targets {
            match m.artifact(tool, v, t) {
                Some(a) => {
                    if !b.arts.iter().any(|(x, _)| {
                        x.tool == a.tool && x.version == a.version && x.target == a.target
                    }) {
                        b.arts.push((a.clone(), t.clone()));
                    }
                }
                None => b.missing.push((tool.clone(), v.clone(), t.clone())),
            }
        }
    }
    b
}

fn out_error(out: &Path, why: impl Into<String>) -> InstallerError {
    InstallerError::new(
        "E1839",
        format!("cannot write the bundle {}: {}", out.display(), why.into()),
    )
    .with_fix("ciginstaller bundle --out carton.tar")
    .with_exit(EXIT_USAGE)
}

pub fn run(cx: &mut Cx, a: &BundleArgs) -> Result<Report, InstallerError> {
    if cx.globals.offline {
        return Err(usage(
            "a bundle is built from downloads, and --offline allows no request",
            "ciginstaller bundle --out carton.tar",
        ));
    }
    // the output is checked before a byte is fetched
    let out = std::path::absolute(&a.out).map_err(|e| out_error(&a.out, e.to_string()))?;
    if out.is_dir() {
        return Err(out_error(&out, "it is a directory"));
    }
    match out.parent() {
        Some(p) if p.is_dir() => {}
        _ => return Err(out_error(&out, "its folder is not there")),
    }
    let targets = if a.targets.is_empty() {
        vec![cx.target.clone()]
    } else {
        parse_targets(&a.targets)?
    };

    cx.lock_and_recover()?;
    let m: Manifest = cx.manifest(ManifestMode::Fresh)?.clone();
    let carton = pick_carton(cx, &m, &InstallArgs::carton(a.carton.clone()))?.clone();
    let cv = carton.version.clone();
    if m.is_carton_yanked(&cv) {
        return Err(InstallerError::new(
            "E1803",
            format!("carton {cv} was yanked; a bundle of it would install nowhere"),
        )
        .with_fix("ciginstaller carton list")
        .with_exit(EXIT_REFUSED));
    }
    let tools: Vec<(String, Version)> = carton
        .products
        .iter()
        .filter(|(t, _)| t.as_str() != "ciginstaller")
        .map(|(t, v)| (t.clone(), v.clone()))
        .collect();
    let builds = builds_for(&m, &tools, &targets);
    if let Some((t, v, target)) = builds.missing.first() {
        return Err(InstallerError::new(
            "E1815",
            format!("there is no build of {t} {v} for {target}"),
        )
        .with_fix(format!("ciginstaller carton show {cv}"))
        .with_exit(EXIT_REFUSED));
    }

    // the manifest exactly as it was admitted: `manifest(Fresh)` has just cached those bytes
    let channel = m.channel.clone();
    let dir = manifest_cache_dir(&cx.home, &channel);
    let doc = std::fs::read(dir.join("manifest.toml"))
        .map_err(|e| bug(format!("the admitted manifest is not in the cache: {e}")))?;
    let side = std::fs::read(dir.join("manifest.toml.sha256")).map_err(|e| {
        bug(format!(
            "the admitted manifest's checksum is not in the cache: {e}"
        ))
    })?;
    let mut files = vec![
        BundleFile::bytes(format!("manifest-{channel}.toml"), doc),
        BundleFile::bytes(format!("manifest-{channel}.toml.sha256"), side),
    ];

    let _ = sweep_downloads(&cx.home);
    let mut staged_paths: Vec<PathBuf> = Vec::new();
    let result = (|| -> Result<(), InstallerError> {
        for (art, target) in &builds.arts {
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
            staged_paths.push(staged.path.clone());
            let (asset_loc, side_loc) = artifact_locations(&staged.art);
            let (asset_name, side_name) = (asset_loc.file_name(), side_loc.file_name());
            if !plain(&asset_name) || !plain(&side_name) {
                return Err(out_error(
                    &out,
                    format!("{asset_name} is not a file name a bundle can hold"),
                ));
            }
            files.push(BundleFile {
                name: asset_name,
                data: BundleData::File(staged.path.clone()),
                sha256: staged.verified.sha256.clone(),
                size: staged.art.size,
            });
            if let Some(text) = &staged.sidecar {
                files.push(BundleFile::bytes(side_name, text.clone().into_bytes()));
            }
        }
        // the same file from two builds (two targets served by one asset) is held once
        files.sort_by(|x, y| x.name.cmp(&y.name));
        files.dedup_by(|x, y| x.name == y.name && x.sha256 == y.sha256);
        let meta = [
            ("channel", json!(channel)),
            ("sequence", json!(m.sequence)),
            ("carton", json!(cv.to_string())),
            (
                "targets",
                json!(targets.iter().map(|t| t.to_string()).collect::<Vec<_>>()),
            ),
        ];
        write_bundle(&out, &meta, files.clone()).map_err(|e: BundleError| {
            let code = e.code();
            let mut err = InstallerError::new(code, e.to_string()).with_exit(EXIT_REFUSED);
            err = err.with_fix(if code == "E1836" {
                "ciginstaller gc --dry-run"
            } else {
                "ciginstaller bundle --out carton.tar"
            });
            err
        })
    })();
    for p in &staged_paths {
        let _ = std::fs::remove_file(p);
    }
    result?;

    let bytes = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    let names: Vec<String> = files.iter().map(|f| f.name.clone()).collect();
    let quoted = {
        let s = out.display().to_string();
        if s.contains(char::is_whitespace) {
            format!("\"{s}\"")
        } else {
            s
        }
    };
    Ok(Report::done(format!(
        "bundle {} written: carton {cv} for {}",
        out.display(),
        targets
            .iter()
            .map(|t| t.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    ))
    .detail(format!(
        "{} builds, {} files, {}; every build was admitted before it went in",
        builds.arts.len(),
        names.len() + 1,
        size_text(bytes)
    ))
    .detail(
        "unsigned: a bundle carries no signature until #9 lands, so it is one source; installing \
         it reaches pinned for builds this ciginstaller has pins for, and is otherwise refused \
         unless allow_single_source is set",
    )
    .next(format!("ciginstaller install carton --from {quoted}"))
    .data(json!({
        "out": out.display().to_string(),
        "carton": cv.to_string(),
        "channel": channel,
        "sequence": m.sequence,
        "targets": targets.iter().map(|t| t.to_string()).collect::<Vec<_>>(),
        "builds": builds.arts.len(),
        "files": files.iter().map(|f| json!({"name": f.name, "size": f.size, "sha256": f.sha256})).collect::<Vec<_>>(),
        "bytes": bytes,
        "signed": false,
    })))
}

// ---------------------------------------------------------------- install --from <file.tar>

/// A bundle unpacked inside the home for one `install`: removed when this drops, and the
/// `bundle-*` leftovers of a run that was killed are swept when the next one opens.
pub struct OpenBundle {
    dir: PathBuf,
    /// The channels that have a manifest in the bundle.
    pub channels: Vec<String>,
    /// The carton the bundle was made for (its index says; the index decides nothing but which
    /// carton `install` means when none was named, and the manifest still has to admit it).
    pub carton: Option<String>,
}

impl Drop for OpenBundle {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn bundle_error(file: &Path, e: BundleError) -> InstallerError {
    let code = e.code();
    InstallerError::new(code, format!("{}: {e}", file.display()))
        .with_hint("nothing was installed")
        .with_fix(if code == "E1836" {
            "ciginstaller gc --dry-run"
        } else {
            "ciginstaller bundle --out carton.tar"
        })
        .with_exit(if code == "E1836" {
            cigi_ui::EXIT_FAILED
        } else {
            EXIT_REFUSED
        })
}

/// Unpack `file` into a private directory in the home and make it this run's source. The lock
/// must be held (`install` takes it first). The files in it are only as good as the manifest
/// inside says: every one of them still goes through the same admission as a download.
pub fn open_bundle(cx: &mut Cx, file: &Path) -> Result<OpenBundle, InstallerError> {
    let cache = cx.home.installer().join("cache");
    if let Ok(rd) = std::fs::read_dir(&cache) {
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().starts_with("bundle-") {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
    let dir = cache.join(format!(
        "bundle-{}-{}",
        std::process::id(),
        cigi_source::digest::random_suffix()
    ));
    std::fs::create_dir(&dir).map_err(|e| {
        InstallerError::new("E1835", format!("cannot create {}: {e}", dir.display()))
    })?;
    // from here the guard removes the directory, whatever happens
    let mut open = OpenBundle {
        dir: dir.clone(),
        channels: Vec::new(),
        carton: None,
    };
    let unpacked = cigi_source::unpack_bundle(file, &dir).map_err(|e| bundle_error(file, e))?;
    open.channels = unpacked.channels();
    open.carton = unpacked
        .index
        .get("carton")
        .and_then(|v| v.as_str())
        .map(String::from);
    let label = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "bundle".into());
    cx.source = Box::new(cigi_source::BundleSource::new(&dir, &label).map_err(ie)?);
    Ok(open)
}
