// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `programs [--project DIR] [--json]` and `programs rebuild <name> [--script FILE] [--dry-run]` (#30).

use cigi_ui::InstallerError;

use super::Outcome;
use crate::cli::{ProgramsCmd, ProgramsSub};
use crate::cx::Cx;
use crate::flows::programs::{self, RebuildArgs};

pub fn run(cx: &mut Cx, c: &ProgramsCmd) -> Result<Outcome, InstallerError> {
    match &c.sub {
        None => programs::list(cx, &c.project).map(Outcome::Report),
        Some(ProgramsSub::Rebuild {
            target,
            script,
            dry_run,
        }) => programs::rebuild(
            cx,
            &RebuildArgs {
                target: target.clone(),
                script: script.clone(),
                projects: c.project.clone(),
                dry_run: *dry_run,
            },
        )
        .map(Outcome::Report),
    }
}
