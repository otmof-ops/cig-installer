// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `gc [--dry-run] [--keep-days N]`.

use cigi_ui::InstallerError;

use super::Outcome;
use crate::cx::Cx;
use crate::flows::gc::{self, GcArgs};

pub fn run(cx: &mut Cx, dry_run: bool, keep_days: Option<u32>) -> Result<Outcome, InstallerError> {
    gc::run(cx, &GcArgs { dry_run, keep_days }).map(Outcome::Report)
}
