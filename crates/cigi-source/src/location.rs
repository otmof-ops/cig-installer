// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0
//! What a `Source` is asked for. A location names a thing; the source decides where it lives.

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
