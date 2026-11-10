// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! Trust (section 7). Everything that installs takes `Admitted<..>`, and only code in this
//! directory can make one (`seal`); a repo test greps for that. `DigestTrust` is today's
//! implementation; a `TufTrust` later implements the same `Trust` trait.

mod attest;
mod digest_trust;

pub use attest::{Attest, GhAttest, NoAttest, StubAttest};
pub use digest_trust::DigestTrust;

use crate::error::TrustError;
use cig_compat::{Artifact, Manifest};
use std::ops::Deref;
use std::path::Path;

/// How far a file was proven. Ordered: each level implies the ones below. Section 7.3 adds
/// `Signed` above `Pinned`; `non_exhaustive` so adding it breaks no match.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TrustLevel {
    Checksummed,
    Attested,
    Pinned,
}
impl TrustLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            TrustLevel::Checksummed => "checksummed",
            TrustLevel::Attested => "attested",
            TrustLevel::Pinned => "pinned",
        }
    }
    /// The `trust` config values.
    pub fn parse(s: &str) -> Option<TrustLevel> {
        match s.trim() {
            "checksummed" => Some(TrustLevel::Checksummed),
            "attested" => Some(TrustLevel::Attested),
            "pinned" => Some(TrustLevel::Pinned),
            _ => None,
        }
    }
}

/// What is known about the manifest the machine saw last on this channel.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SeenState {
    /// Highest `sequence` seen on this channel; `None` on first use (trust on first use).
    pub sequence: Option<u64>,
    /// `--offline`: an expired manifest is allowed, with a note.
    pub offline: bool,
    /// Seconds since the epoch to use as "now" (tests, and `CIGI_NOW` under fault-injection).
    pub now: Option<i64>,
    /// The channel that was asked for; a manifest that names another channel is refused (E1825),
    /// so a `beta` manifest cannot be replayed as `stable`.
    pub channel: Option<String>,
    /// A private (0700) place for the temporary files handed to `gh`; `installer/cache` of the home.
    pub scratch: Option<std::path::PathBuf>,
}

/// A value a `Trust` implementation vouched for. Fields are private; `seal` is the only way in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Admitted<T> {
    value: T,
    notes: Vec<String>,
}

/// Only the `trust` directory calls this.
pub fn seal<T>(t: T, _by: &dyn Trust) -> Admitted<T> {
    Admitted {
        value: t,
        notes: Vec::new(),
    }
}

impl<T> Admitted<T> {
    pub fn get(&self) -> &T {
        &self.value
    }
    pub fn into_inner(self) -> T {
        self.value
    }
    /// One-line facts the caller may show: `attestation verified`, `expired, used offline`.
    pub fn notes(&self) -> &[String] {
        &self.notes
    }
    fn note(mut self, n: impl Into<String>) -> Self {
        self.notes.push(n.into());
        self
    }
}
impl<T> Deref for Admitted<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.value
    }
}
impl Admitted<Verified> {
    /// The source (not the trust) knows whether the two checksummed places were independent.
    pub fn with_independent(mut self, independent: bool) -> Self {
        self.value.independent = independent;
        self
    }
}

/// The facts behind an admitted artifact; they go into the receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verified {
    pub level: TrustLevel,
    pub sha256: String,
    /// Were the manifest and the sidecar served by different places? Set by the caller from
    /// `Source::independent()`; `DigestTrust` starts it at true.
    pub independent: bool,
    pub attested_by: Option<String>,
}

pub trait Trust {
    fn admit_manifest(
        &self,
        doc: &[u8],
        sidecar: Option<&str>,
        attestation: Option<&[u8]>,
        seen: &SeenState,
    ) -> Result<Admitted<Manifest>, TrustError>;
    fn admit_artifact(
        &self,
        art: &Artifact,
        sidecar: Option<&str>,
        file: &Path,
    ) -> Result<Admitted<Verified>, TrustError>;
    fn level(&self) -> &'static str;
    /// May an artifact from a single-operator source (a mirror) stay at `checksummed`?
    /// Off unless the user said so (`allow_single_source = true`).
    fn allows_single_source(&self) -> bool {
        false
    }
}
