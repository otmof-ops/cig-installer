// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! Library errors become `InstallerError`s here, one place, so every flow reports the same way.

use cig_compat::ManifestError;
use cigi_home::{
    HomeError, HumidorError, JournalError, LockError, PathError, ShimError, StateError, StoreError,
    ToolchainError,
};
use cigi_source::{ArchiveError, FetchError, TrustError};
use cigi_ui::InstallerError;

/// An error that carries an `E18xx` code and a message.
pub trait Coded: std::fmt::Display {
    fn error_code(&self) -> &'static str;
}

macro_rules! coded {
    ($($t:ty),*) => { $(impl Coded for $t { fn error_code(&self) -> &'static str { self.code() } })* };
}
coded!(
    HomeError,
    LockError,
    StoreError,
    JournalError,
    ToolchainError,
    StateError,
    ShimError,
    PathError,
    HumidorError,
    FetchError,
    TrustError,
    ArchiveError
);

impl Coded for ManifestError {
    fn error_code(&self) -> &'static str {
        self.code
    }
}

/// `.map_err(ie)`: the library error as an installer error with its own code.
pub fn ie<E: Coded>(e: E) -> InstallerError {
    InstallerError::new(e.error_code(), e.to_string())
}

/// A bug: something that should have been impossible.
pub fn bug(what: impl Into<String>) -> InstallerError {
    InstallerError::new("E1890", what)
}
