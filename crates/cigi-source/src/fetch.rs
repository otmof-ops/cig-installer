// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0
//! Getting a manifest and an artifact: fetch, check, and only then hand over.
//!
//! The order for an artifact is: sidecar (small, cheap), the asset into `<asset>.partial-<pid>`
//! under `installer/cache/downloads/`, size then digest then sidecar then floor in `admit_artifact`,
//! and only after all of that a rename to the final name. Any failure removes the partial file.
//! Nothing is unpacked here: `stage_artifact` does that, from a file that was admitted.

use crate::archive::unpack_tar_gz;
use crate::clock;
use crate::error::{FetchError, TrustError};
use crate::location::{is_plain_segment, Location, ManifestPart};
use crate::progress::Progress;
use crate::source::Source;
use crate::trust::{Admitted, SeenState, Trust, Verified};
use cig_compat::{Artifact, Layout, Manifest, Target, Version};
use cigi_home::{Home, StagedFile};
use std::path::{Path, PathBuf};

const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
const MAX_SIDECAR_BYTES: usize = 4096;
const MAX_ATTESTATION_BYTES: usize = 1024 * 1024;

fn io_err(path: &Path) -> impl FnOnce(std::io::Error) -> FetchError + '_ {
    move |source| FetchError::Io {
        path: path.to_path_buf(),
        source,
    }
}

pub fn downloads_dir(home: &Home) -> PathBuf {
    home.installer().join("cache").join("downloads")
}

pub fn manifest_cache_dir(home: &Home, channel: &str) -> PathBuf {
    home.installer()
        .join("cache")
        .join("manifests")
        .join(channel)
}

/// Remove everything in `cache/downloads/` (the flows call this at the start of every install).
pub fn sweep_downloads(home: &Home) -> Result<usize, FetchError> {
    let dir = downloads_dir(home);
    let rd = match std::fs::read_dir(&dir) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(io_err(&dir)(e)),
    };
    let mut n = 0;
    for e in rd.flatten() {
        if std::fs::remove_file(e.path()).is_ok() {
            n += 1;
        }
    }
    Ok(n)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), FetchError> {
    use std::io::Write;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.tmp-{}", std::process::id()));
    let r = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r.map_err(io_err(path))
}

/// Removes a path on drop unless disarmed.
struct Cleanup(Option<PathBuf>);
impl Cleanup {
    fn disarm(&mut self) {
        self.0 = None;
    }
}
impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Some(p) = self.0.take() {
            let _ = std::fs::remove_file(p);
        }
    }
}

// ---------------------------------------------------------------- manifests

#[derive(Debug, Clone, Default)]
pub struct ManifestOpts {
    pub offline: bool,
    /// Highest sequence the caller knows (`state.json`); the cache's own is also used, the higher wins.
    pub seen_sequence: Option<u64>,
    /// "Now" in epoch seconds (tests; `CIGI_NOW` under fault-injection).
    pub now: Option<i64>,
    /// Look for the attestation bundle next to the manifest. Off saves a request when `gh` is absent.
    pub fetch_attestation: bool,
}

/// What `fetched.json` records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedMeta {
    pub sequence: u64,
    pub fetched_at: String,
    pub expires: String,
    pub source: String,
    pub sha256: String,
}

fn check_channel(channel: &str) -> Result<(), FetchError> {
    if is_plain_segment(channel) {
        Ok(())
    } else {
        Err(FetchError::Refused {
            url: channel.to_string(),
            why: "not a channel name".into(),
        })
    }
}

/// The cached manifest's metadata, if there is a cache directory with a readable `fetched.json`.
pub fn read_fetched(home: &Home, channel: &str) -> Option<FetchedMeta> {
    let text =
        std::fs::read_to_string(manifest_cache_dir(home, channel).join("fetched.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(String::from);
    Some(FetchedMeta {
        sequence: v.get("sequence")?.as_u64()?,
        fetched_at: s("fetched_at")?,
        expires: s("expires").unwrap_or_default(),
        source: s("source").unwrap_or_default(),
        sha256: s("sha256").unwrap_or_default(),
    })
}

/// Seconds since the cached manifest was fetched; `None` with no cache. (The 24 hour rule of 12.2 is the caller's.)
pub fn cache_age_secs(home: &Home, channel: &str, now: Option<i64>) -> Option<i64> {
    let m = read_fetched(home, channel)?;
    Some(now.unwrap_or_else(clock::now) - clock::parse(&m.fetched_at)?)
}

struct Cached {
    doc: Vec<u8>,
    sidecar: String,
    meta: FetchedMeta,
}

fn read_cache(home: &Home, channel: &str) -> Option<Cached> {
    let dir = manifest_cache_dir(home, channel);
    Some(Cached {
        doc: std::fs::read(dir.join("manifest.toml")).ok()?,
        sidecar: std::fs::read_to_string(dir.join("manifest.toml.sha256")).ok()?,
        meta: read_fetched(home, channel)?,
    })
}

/// Fetch (or, offline, load) and admit the manifest of `channel`. The cache is written only after
/// admission succeeds. Section 2.3's signature; `get_manifest_with` takes the full option set.
pub fn get_manifest(
    src: &dyn Source,
    trust: &dyn Trust,
    home: &Home,
    channel: &str,
    offline: bool,
) -> Result<Admitted<Manifest>, FetchError> {
    let opts = ManifestOpts {
        offline,
        fetch_attestation: crate::trust::GhAttest.available_cached(),
        ..ManifestOpts::default()
    };
    get_manifest_with(src, trust, home, channel, &opts)
}

pub fn get_manifest_with(
    src: &dyn Source,
    trust: &dyn Trust,
    home: &Home,
    channel: &str,
    opts: &ManifestOpts,
) -> Result<Admitted<Manifest>, FetchError> {
    check_channel(channel)?;
    let cached = read_cache(home, channel);
    let seen_sequence = match (opts.seen_sequence, cached.as_ref().map(|c| c.meta.sequence)) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    };
    let seen = SeenState {
        sequence: seen_sequence,
        offline: opts.offline,
        now: opts.now,
        channel: Some(channel.to_string()),
    };
    if opts.offline {
        let Some(c) = cached else {
            return Err(FetchError::NoManifest {
                channel: channel.to_string(),
            });
        };
        return Ok(trust.admit_manifest(&c.doc, Some(&c.sidecar), None, &seen)?);
    }
    let at = |part| Location::Manifest {
        channel: channel.to_string(),
        part,
    };
    let doc = src.fetch_text(&at(ManifestPart::Doc), MAX_MANIFEST_BYTES)?;
    let sidecar = match src.fetch_text(&at(ManifestPart::Sidecar), MAX_SIDECAR_BYTES) {
        Ok(t) => Some(t),
        Err(e) if e.is_not_found() => None,
        Err(e) => return Err(e),
    };
    let attestation = if opts.fetch_attestation {
        src.fetch_text(&at(ManifestPart::Attestation), MAX_ATTESTATION_BYTES)
            .ok()
    } else {
        None
    };
    let admitted = trust.admit_manifest(
        doc.as_bytes(),
        sidecar.as_deref(),
        attestation.as_deref().map(str::as_bytes),
        &seen,
    )?;
    // cache only what passed
    let dir = manifest_cache_dir(home, channel);
    std::fs::create_dir_all(&dir).map_err(io_err(&dir))?;
    let side = sidecar.unwrap_or_default();
    let meta = format!(
        "{{\n  \"format\": 1,\n  \"sequence\": {},\n  \"fetched_at\": {},\n  \"expires\": {},\n  \"source\": {},\n  \"sha256\": {}\n}}\n",
        admitted.sequence,
        json_str(&clock::now_rfc3339()),
        json_str(&admitted.expires),
        json_str(&src.name()),
        json_str(&crate::digest::sha256_hex(doc.as_bytes())),
    );
    write_atomic(&dir.join("manifest.toml.sha256"), side.as_bytes())?;
    write_atomic(&dir.join("fetched.json"), meta.as_bytes())?;
    write_atomic(&dir.join("manifest.toml"), doc.as_bytes())?;
    Ok(admitted)
}

fn json_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

// ---------------------------------------------------------------- artifacts

/// A download that passed `admit_artifact`: the verified file in `cache/downloads/` and the facts about it.
#[derive(Debug, Clone)]
pub struct StagedArtifact {
    pub art: Artifact,
    pub path: PathBuf,
    pub verified: Admitted<Verified>,
    /// `Source::name()` of where it came from (goes in the receipt's `origin`).
    pub origin: String,
}

fn asset_locations(art: &Artifact) -> (Location, Location) {
    if let Some(url) = &art.url {
        let base = url.rsplit_once('/').map(|(b, _)| b).unwrap_or_default();
        return (
            Location::Url { url: url.clone() },
            Location::Url {
                url: format!("{base}/{}", art.sidecar),
            },
        );
    }
    let mk = |name: &str| {
        if art.tool == "ciginstaller" {
            Location::Installer {
                tag: art.tag.clone(),
                name: name.to_string(),
            }
        } else {
            Location::Asset {
                repo: art.repo.clone(),
                tag: art.tag.clone(),
                name: name.to_string(),
            }
        }
    };
    (mk(&art.asset), mk(&art.sidecar))
}

pub fn get_artifact(
    src: &dyn Source,
    trust: &dyn Trust,
    m: &Manifest,
    tool: &str,
    v: &Version,
    home: &Home,
    p: &mut dyn Progress,
) -> Result<StagedArtifact, FetchError> {
    get_artifact_for(src, trust, m, tool, v, &Target::host(), home, p)
}

#[allow(clippy::too_many_arguments)]
pub fn get_artifact_for(
    src: &dyn Source,
    trust: &dyn Trust,
    m: &Manifest,
    tool: &str,
    v: &Version,
    target: &Target,
    home: &Home,
    p: &mut dyn Progress,
) -> Result<StagedArtifact, FetchError> {
    let art = m
        .artifact(tool, v, target)
        .ok_or_else(|| FetchError::NoBuild {
            tool: tool.to_string(),
            version: v.to_string(),
            target: target.to_string(),
        })?
        .clone();
    if !is_plain_segment(&art.asset) {
        return Err(TrustError::Malformed(format!(
            "asset name {:?} is not a plain file name",
            art.asset
        ))
        .into());
    }
    let dir = downloads_dir(home);
    std::fs::create_dir_all(&dir).map_err(io_err(&dir))?;
    let partial = dir.join(format!("{}.partial-{}", art.asset, std::process::id()));
    let mut guard = Cleanup(Some(partial.clone()));
    let (asset_loc, sidecar_loc) = asset_locations(&art);
    let sidecar = match src.fetch_text(&sidecar_loc, MAX_SIDECAR_BYTES) {
        Ok(t) => Some(t),
        Err(e) if e.is_not_found() => None,
        Err(e) => return Err(e),
    };
    src.fetch(&asset_loc, &partial, p)?;
    let verified = trust
        .admit_artifact(&art, sidecar.as_deref(), &partial)?
        .with_independent(src.independent());
    let path = dir.join(&art.asset);
    let _ = std::fs::remove_file(&path);
    std::fs::rename(&partial, &path).map_err(io_err(&path))?;
    guard.disarm();
    Ok(StagedArtifact {
        art,
        path,
        verified,
        origin: src.name(),
    })
}

/// Lay an admitted download out as files in `staging` (an empty directory the caller owns):
/// a `binary` is copied to `bin/<name>` (the tool's first shim path, executable); an `archive` is
/// unpacked with `unpack_tar_gz` and the tool's `strip`.
pub fn stage_artifact(
    m: &Manifest,
    staged: &StagedArtifact,
    staging: &Path,
) -> Result<Vec<StagedFile>, FetchError> {
    match staged.art.layout {
        Layout::Archive => Ok(unpack_tar_gz(&staged.path, staged.art.strip, staging)?),
        Layout::Binary => {
            let shim = m.tools.get(&staged.art.tool).and_then(|t| t.shims.first());
            let target_is_windows = staged.art.target.starts_with("windows");
            let rel_text = match shim {
                Some(s) if target_is_windows => {
                    s.windows_path.clone().unwrap_or_else(|| s.path.clone())
                }
                Some(s) => s.path.clone(),
                None => format!("bin/{}", staged.art.tool),
            };
            let rel: PathBuf = rel_text.split('/').collect();
            if rel
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
            {
                return Err(TrustError::Malformed(format!(
                    "shim path {rel_text:?} is not relative and plain"
                ))
                .into());
            }
            let dest = staging.join(&rel);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).map_err(io_err(parent))?;
            }
            std::fs::copy(&staged.path, &dest).map_err(io_err(&dest))?;
            Ok(vec![StagedFile {
                path: rel,
                source: dest,
                exec: true,
            }])
        }
    }
}
