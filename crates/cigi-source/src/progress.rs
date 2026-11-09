// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0
//! The progress seam. `cigi-ui` does not depend on this crate, so `ciginstaller` adapts the
//! UI's bar to this trait (a ten-line wrapper); `NoProgress` is for tests and quiet paths.

pub trait Progress {
    /// The total is known (Content-Length or file size). May be called at most once.
    fn set_total(&mut self, total: u64);
    /// `bytes` more arrived.
    fn advance(&mut self, bytes: u64);
    /// The transfer ended (success or not).
    fn done(&mut self);
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NoProgress;
impl Progress for NoProgress {
    fn set_total(&mut self, _total: u64) {}
    fn advance(&mut self, _bytes: u64) {}
    fn done(&mut self) {}
}

/// Counts what it is told; used by tests.
#[derive(Debug, Default, Clone)]
pub struct CountingProgress {
    pub total: Option<u64>,
    pub advanced: u64,
    pub finished: bool,
}
impl Progress for CountingProgress {
    fn set_total(&mut self, total: u64) {
        self.total = Some(total);
    }
    fn advance(&mut self, bytes: u64) {
        self.advanced += bytes;
    }
    fn done(&mut self) {
        self.finished = true;
    }
}
