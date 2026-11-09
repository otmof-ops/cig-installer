// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0

//! `cig-compat`: pure versions, pins, the carton manifest and the marry-up
//! checks. This is the one place that answers "which cigo is in carton X?" and
//! "does this combination fit?". No I/O except reading text handed to it.

#![forbid(unsafe_code)]

pub mod check;
pub mod manifest;
mod time;
pub mod versions;

pub use check::*;
pub use manifest::*;
pub use time::rfc3339_to_epoch;
pub use versions::{CartonVersion, Pin, PinError, Version, VersionError};
