// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! Errors. Each type has `code()` returning the registered `E18xx` (section 11.3); the message
//! says what happened in plain words and is what `cigi-ui` shows under the code.

use std::fmt;
use std::path::PathBuf;

/// Why a sidecar could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidecarError {
    Empty,
    /// The first token is not 64 hex digits.
    NotADigest(String),
}
impl fmt::Display for SidecarError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SidecarError::Empty => f.write_str("the checksum file is empty"),
            SidecarError::NotADigest(t) => write!(
                f,
                "the checksum file does not start with a sha256 digest (found {t:?})"
            ),
        }
    }
}
impl std::error::Error for SidecarError {}

/// Why a trust check refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrustError {
    /// E1821: the file's digest is not the one the manifest (or a pin) names.
    DigestMismatch {
        what: String,
        expected: String,
        actual: String,
    },
    /// E1821: the download is not the size the manifest names.
    SizeMismatch {
        what: String,
        expected: u64,
        actual: u64,
    },
    /// E1822
    FloorNotMet {
        floor: &'static str,
        reached: &'static str,
        why: String,
    },
    /// E1900: a build came over plain http (loopback only) without a pin to vouch for it.
    PlainHttp { why: String },
    /// E1823
    Expired { expires: String },
    /// E1824
    SequenceBackwards { seen: u64, got: u64 },
    /// E1825
    Malformed(String),
    /// E1827: no checksum file next to the product's release.
    SidecarMissing { what: String },
    /// E1827: the product's own checksum file names another digest.
    SidecarDisagrees {
        what: String,
        manifest: String,
        sidecar: String,
    },
    /// E1828
    AttestationFailed { what: String, why: String },
    /// E1820: the file could not be read to check it.
    Io { what: String, why: String },
}
impl TrustError {
    pub fn code(&self) -> &'static str {
        match self {
            TrustError::DigestMismatch { .. } | TrustError::SizeMismatch { .. } => "E1821",
            TrustError::FloorNotMet { .. } => "E1822",
            TrustError::PlainHttp { .. } => "E1900",
            TrustError::Expired { .. } => "E1823",
            TrustError::SequenceBackwards { .. } => "E1824",
            TrustError::Malformed(_) => "E1825",
            TrustError::SidecarMissing { .. } | TrustError::SidecarDisagrees { .. } => "E1827",
            TrustError::AttestationFailed { .. } => "E1828",
            TrustError::Io { .. } => "E1820",
        }
    }
}
impl fmt::Display for TrustError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TrustError::DigestMismatch {
                what,
                expected,
                actual,
            } => {
                write!(f, "{what} has sha256 {actual}, expected {expected}")
            }
            TrustError::SizeMismatch {
                what,
                expected,
                actual,
            } => {
                write!(f, "{what} is {actual} bytes, the manifest says {expected}")
            }
            TrustError::FloorNotMet {
                floor,
                reached,
                why,
            } => {
                write!(
                    f,
                    "trust is set to \"{floor}\" but this file only reached \"{reached}\": {why}"
                )
            }
            TrustError::PlainHttp { why } => f.write_str(why),
            TrustError::Expired { expires } => write!(f, "the manifest expired on {expires}"),
            TrustError::SequenceBackwards { seen, got } => {
                write!(
                    f,
                    "the manifest's sequence went backwards ({got} after {seen})"
                )
            }
            TrustError::Malformed(why) => write!(f, "the manifest is malformed: {why}"),
            TrustError::SidecarMissing { what } => {
                write!(f, "{what} has no checksum file from its own release")
            }
            TrustError::SidecarDisagrees {
                what,
                manifest,
                sidecar,
            } => {
                write!(f, "{what}: the manifest says {manifest} but the release's own checksum file says {sidecar}")
            }
            TrustError::AttestationFailed { what, why } => {
                write!(f, "attestation check for {what} failed: {why}")
            }
            TrustError::Io { what, why } => write!(f, "could not read {what}: {why}"),
        }
    }
}
impl std::error::Error for TrustError {}

/// Why unpacking refused. Every variant but `Io` is E1826.
#[derive(Debug)]
pub enum ArchiveError {
    /// An entry that would land outside the target, or is not a plain file or directory.
    Unsafe { entry: String, why: &'static str },
    /// A limit was passed (256 MB per file, 20 000 entries, 1 GiB in all).
    TooBig { why: String },
    /// Not a readable tar.gz.
    Unreadable(String),
    /// The disk said no.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}
impl ArchiveError {
    pub fn code(&self) -> &'static str {
        match self {
            ArchiveError::Io { source, .. } if source.raw_os_error() == Some(28) => "E1836",
            ArchiveError::Io { .. } => "E1820",
            _ => "E1826",
        }
    }
}
impl fmt::Display for ArchiveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArchiveError::Unsafe { entry, why } => {
                write!(f, "archive entry {entry:?} refused: {why}")
            }
            ArchiveError::TooBig { why } => write!(f, "archive refused: {why}"),
            ArchiveError::Unreadable(why) => write!(f, "archive could not be read: {why}"),
            ArchiveError::Io { path, source } => {
                write!(f, "could not write {}: {source}", path.display())
            }
        }
    }
}
impl std::error::Error for ArchiveError {}

/// Anything a fetch can fail on.
#[derive(Debug)]
pub enum FetchError {
    /// E1820: the transfer failed (connect, TLS, timeout, short body, bad status).
    Network {
        url: String,
        why: String,
    },
    /// E1820: the server (or directory) has no such file. Kept apart so a missing sidecar can be E1827.
    NotFound {
        url: String,
    },
    /// E1820: refused before any request (plain http to a non-loopback host, redirect to another host, bad name).
    Refused {
        url: String,
        why: String,
    },
    /// E1900: plain http to a host that is not loopback, or an https request sent on to http.
    PlainHttp {
        url: String,
        why: String,
    },
    /// E1820: more bytes than the limit.
    TooLarge {
        url: String,
        limit: u64,
    },
    /// E1820 or E1836: local I/O.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// E1815: the manifest has no build of this tool and version for this platform.
    NoBuild {
        tool: String,
        version: String,
        target: String,
    },
    /// E1810: offline (or the network failed) and nothing cached.
    NoManifest {
        channel: String,
    },
    /// E1873: the step needs the network and offline was requested.
    Offline {
        what: String,
    },
    Trust(TrustError),
    Archive(ArchiveError),
}
impl FetchError {
    pub fn code(&self) -> &'static str {
        match self {
            FetchError::PlainHttp { .. } => "E1900",
            FetchError::Network { .. }
            | FetchError::NotFound { .. }
            | FetchError::Refused { .. }
            | FetchError::TooLarge { .. } => "E1820",
            FetchError::Io { source, .. } if source.raw_os_error() == Some(28) => "E1836",
            FetchError::Io { .. } => "E1820",
            FetchError::NoBuild { .. } => "E1815",
            FetchError::NoManifest { .. } => "E1810",
            FetchError::Offline { .. } => "E1873",
            FetchError::Trust(t) => t.code(),
            FetchError::Archive(a) => a.code(),
        }
    }
    pub fn is_not_found(&self) -> bool {
        matches!(self, FetchError::NotFound { .. })
    }
}
impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FetchError::Network { url, why } => write!(f, "download of {url} failed: {why}"),
            FetchError::NotFound { url } => write!(f, "{url} does not exist (404)"),
            FetchError::Refused { url, why } | FetchError::PlainHttp { url, why } => {
                write!(f, "refused to fetch {url}: {why}")
            }
            FetchError::TooLarge { url, limit } => {
                write!(f, "{url} is larger than the {limit}-byte limit")
            }
            FetchError::Io { path, source } => write!(f, "{}: {source}", path.display()),
            FetchError::NoBuild {
                tool,
                version,
                target,
            } => write!(f, "no build of {tool} {version} for {target}"),
            FetchError::NoManifest { channel } => write!(
                f,
                "no manifest for channel {channel}: nothing cached and no network"
            ),
            FetchError::Offline { what } => {
                write!(f, "{what} needs the network, and offline was requested")
            }
            FetchError::Trust(t) => t.fmt(f),
            FetchError::Archive(a) => a.fmt(f),
        }
    }
}
impl std::error::Error for FetchError {}
impl From<TrustError> for FetchError {
    fn from(t: TrustError) -> Self {
        FetchError::Trust(t)
    }
}
impl From<ArchiveError> for FetchError {
    fn from(a: ArchiveError) -> Self {
        FetchError::Archive(a)
    }
}
