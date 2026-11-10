// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! `cig-compat`: pure versions, pins, the carton manifest and the marry-up
//! checks. This is the one place that answers "which cigo is in carton X?" and
//! "does this combination fit?". No I/O except reading text, or a reader, handed to it.

#![forbid(unsafe_code)]

pub mod archive;
pub mod check;
pub mod manifest;
pub mod program;
mod time;
pub mod versions;

pub use archive::{
    prune_plan, ArchiveAsset, ArchiveEntry, ArchiveIndex, ArchiveRef, ArchiveTag, PlanRow,
};
pub use check::*;
pub use manifest::*;
pub use program::Program;
pub use time::rfc3339_to_epoch;
pub use versions::{CartonVersion, Pin, PinError, Version, VersionError};
