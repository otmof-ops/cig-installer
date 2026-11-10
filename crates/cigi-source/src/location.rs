// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! What a `Source` is asked for. A location names a thing; the source decides where it lives.

use crate::error::FetchError;
use crate::fetch::artifact_locations;
use crate::source::Source;
use cig_compat::{ArchiveIndex, Artifact};

/// Which of the three files of a published manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestPart {
    /// `manifest-<channel>.toml`
    Doc,
    /// `manifest-<channel>.toml.sha256`
    Sidecar,
    /// `manifest-<channel>.toml.attestation.jsonl` (the GitHub attestation bundle, when published)
    Attestation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Location {
    /// A file of the manifest release.
    Manifest { channel: String, part: ManifestPart },
    /// A release asset of a product repository: `repo` is `owner/name`.
    Asset {
        repo: String,
        tag: String,
        name: String,
    },
    /// A release asset of the installer's own repository.
    Installer { tag: String, name: String },
    /// An explicit URL from a build's `url` key (section 6.1). Not in the section 2.3 enum:
    /// added because `[build.*].url` has nowhere else to go. A mirror serves it by file name.
    Url { url: String },
}

/// The installer's own repository (manifest release and installer assets).
pub const INSTALLER_REPO: &str = "otmof-ops/cig-installer";
/// The rolling release tag the manifests are published under.
pub const MANIFEST_TAG: &str = "manifest";

impl Location {
    /// The file name this location has on a flat mirror.
    pub fn file_name(&self) -> String {
        match self {
            Location::Manifest { channel, part } => {
                let base = format!("manifest-{channel}.toml");
                match part {
                    ManifestPart::Doc => base,
                    ManifestPart::Sidecar => format!("{base}.sha256"),
                    ManifestPart::Attestation => format!("{base}.attestation.jsonl"),
                }
            }
            Location::Asset { name, .. } | Location::Installer { name, .. } => name.clone(),
            Location::Url { url } => url.rsplit('/').next().unwrap_or_default().to_string(),
        }
    }

    /// `(repo, tag)` on GitHub for the three structured variants.
    pub fn github_parts(&self) -> Option<(String, String)> {
        match self {
            Location::Manifest { .. } => {
                Some((INSTALLER_REPO.to_string(), MANIFEST_TAG.to_string()))
            }
            Location::Asset { repo, tag, .. } => Some((repo.clone(), tag.clone())),
            Location::Installer { tag, .. } => Some((INSTALLER_REPO.to_string(), tag.clone())),
            Location::Url { .. } => None,
        }
    }
}

/// A single path segment: letters, digits and `._+-` only, not starting with a dot.
pub fn is_plain_segment(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('.')
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'+' | b'-'))
}

/// `owner/name` with plain segments.
pub fn is_plain_repo(s: &str) -> bool {
    match s.split_once('/') {
        Some((a, b)) => is_plain_segment(a) && is_plain_segment(b),
        None => false,
    }
}

/// A sidecar is one line; the probe never reads more.
const PROBE_BYTES: usize = 4096;

/// Where an archived build lives: the asset and its sidecar, as explicit URLs of the archive
/// release. `Ok(None)` when the index does not list this build. The pinned bytes win: an index that
/// lists other bytes for the pinned asset is refused, so an archived asset is accepted only at the
/// digest the manifest pins, exactly as before.
pub fn archived_locations(
    art: &Artifact,
    index: &ArchiveIndex,
) -> Result<Option<(Location, Location)>, FetchError> {
    let Some(entry) = index.find(&art.tool, &art.version) else {
        return Ok(None);
    };
    let Some(asset) = entry.asset(&art.asset) else {
        return Ok(None);
    };
    if asset.sha256 != art.sha256 {
        return Err(FetchError::Refused {
            url: asset.url.clone(),
            why: format!(
                "the archive index lists other bytes for {} than the manifest pins",
                art.asset
            ),
        });
    }
    let base = asset
        .url
        .rsplit_once('/')
        .map(|(b, _)| b)
        .unwrap_or_default();
    Ok(Some((
        Location::Url {
            url: asset.url.clone(),
        },
        Location::Url {
            url: format!("{base}/{}", art.sidecar),
        },
    )))
}

/// Where a build is downloaded from (#26), and its sidecar's text when it has one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Located {
    pub asset: Location,
    pub sidecar: Location,
    /// The sidecar as read on the way; `None` when there is no sidecar at that location.
    pub sidecar_text: Option<String>,
}

fn read_sidecar(src: &dyn Source, loc: &Location) -> Result<Option<String>, FetchError> {
    match src.fetch_text(loc, PROBE_BYTES) {
        Ok(t) => Ok(Some(t)),
        Err(e) if e.is_not_found() => Ok(None),
        Err(e) => Err(e),
    }
}

/// The asset and sidecar locations for `art`: the product's repository first, then the archive
/// index. The repository is asked by its sidecar (a few bytes, and the read is kept); only a 404
/// there loads the index, through `index`, and any other answer keeps the repository so its error
/// surfaces where it always did. With no index, or a build the index does not list, the answer is
/// the repository's.
pub fn locate_with_index(
    src: &dyn Source,
    art: &Artifact,
    index: impl FnOnce() -> Result<Option<ArchiveIndex>, FetchError>,
) -> Result<Located, FetchError> {
    let (asset, sidecar) = artifact_locations(art);
    if let Some(text) = read_sidecar(src, &sidecar)? {
        return Ok(Located {
            asset,
            sidecar,
            sidecar_text: Some(text),
        });
    }
    let archived = match index()? {
        Some(index) => archived_locations(art, &index)?,
        None => None,
    };
    match archived {
        Some((asset, sidecar)) => {
            let sidecar_text = read_sidecar(src, &sidecar)?;
            Ok(Located {
                asset,
                sidecar,
                sidecar_text,
            })
        }
        None => Ok(Located {
            asset,
            sidecar,
            sidecar_text: None,
        }),
    }
}
