// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0
//! `self update [--check]` and the hidden `__selfcheck`.

use cigi_ui::InstallerError;

use super::Outcome;
use crate::cx::Cx;
use crate::flows::self_update::{self, SelfArgs};

pub fn update(cx: &mut Cx, check: bool) -> Result<Outcome, InstallerError> {
    self_update::run(cx, &SelfArgs { check }).map(Outcome::Report)
}

/// What a freshly installed binary answers: its name and version on one line, nothing else.
pub fn selfcheck(cx: &mut Cx) -> Result<Outcome, InstallerError> {
    self_update::selfcheck_report(cx).map(Outcome::Page)
}
