// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `repair [--tool <tool>] [--dry-run]`.

use cigi_ui::InstallerError;

use super::Outcome;
use crate::cx::Cx;
use crate::flows::repair::{self, RepairArgs};

pub fn run(cx: &mut Cx, tool: Option<&str>, dry_run: bool) -> Result<Outcome, InstallerError> {
    repair::run(
        cx,
        &RepairArgs {
            tool: tool.map(str::to_string),
            dry_run,
        },
    )
    .map(Outcome::Report)
}
