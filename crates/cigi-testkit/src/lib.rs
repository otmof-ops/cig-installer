// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! Dev-only test support (section 14.1): fixture products with real sha256 sidecars, a mirror
//! directory of releases, a loopback server that records every request, scratch homes, and
//! directory snapshots for "the home is exactly as it was" assertions.
//!
//! Nothing here touches the real HOME or the network. Environment is set for *child processes*
//! only; the test process's own environment is never modified. Failures of the harness itself
//! panic with a message (`must`): this crate is test infrastructure, not shipping code.
//!
//! It does not depend on `cigi-source` (section 2 lists the edge, nothing here needs it, and
//! leaving it out lets `cigi-source` use this crate in its own tests without a dependency cycle).
#![forbid(unsafe_code)]

mod fixture;
mod products;
mod pty;
mod recorder;
mod server;
mod snapshot;
mod tarball;

pub use fixture::{binary_path, output_retrying, Fixture, ManifestPaths, PublishOpts};
pub use products::{asset_name, host_target, ProductBuilder};
pub use pty::script_args;
pub use recorder::{check_no_telemetry, Recorded};
pub use server::{FixtureServer, Rule, ShortBodyServer};
pub use snapshot::{Entry, Snapshot};
pub use tarball::{evil, TarGz};

/// Unwrap-with-a-message for harness code: panics (loudly, with context) instead of `unwrap`.
pub(crate) fn must<T, E: std::fmt::Display>(r: Result<T, E>, what: &str) -> T {
    match r {
        Ok(v) => v,
        Err(e) => panic!("cigi-testkit: {what}: {e}"),
    }
}

/// Lowercase hex sha256.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}
