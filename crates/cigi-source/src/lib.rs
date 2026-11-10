// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! Sources, trust, downloads and safe archive extraction (section 2.3 and 7).
//!
//! Nothing in this crate prints. Every failure carries an `E18xx` code
//! (`code()` on each error type).
#![forbid(unsafe_code)]

pub mod archive;
pub mod bundle;
pub mod clock;
pub mod digest;
pub mod error;
pub mod fetch;
pub mod http;
pub mod location;
pub mod pins;
pub mod progress;
pub mod sidecar;
pub mod source;
pub mod trust;

pub use archive::unpack_tar_gz;
pub use bundle::{
    unpack_bundle, write_bundle, BundleData, BundleError, BundleFile, BundleSource, Unpacked,
};
pub use error::{ArchiveError, FetchError, SidecarError, TrustError};
pub use fetch::{
    get_artifact, get_artifact_for, get_manifest, get_manifest_with, stage_artifact,
    sweep_downloads, ManifestOpts, StagedArtifact,
};
pub use location::{archived_locations, locate_with_index, Located, Location, ManifestPart};
pub use progress::{NoProgress, Progress};
pub use source::{
    source_from_config, try_source_from_config, FetchInfo, GithubSource, MirrorSource, Source,
};
pub use trust::{
    Admitted, Attest, DigestTrust, GhAttest, NoAttest, SeenState, StubAttest, Trust, TrustLevel,
    Verified,
};
