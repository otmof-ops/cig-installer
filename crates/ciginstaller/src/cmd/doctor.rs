// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `doctor [--network] [--tools] [--json]`.

use cigi_ui::InstallerError;

use super::Outcome;
use crate::cx::Cx;
use crate::flows::doctor::{self, DoctorArgs};

pub fn run(cx: &mut Cx, network: bool, tools: bool) -> Result<Outcome, InstallerError> {
    doctor::run(cx, &DoctorArgs { network, tools }).map(Outcome::Report)
}
