// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `uninstall <tool>`, `uninstall --all [--purge] [--keep-journal]` and `remove <carton>`.

use cigi_ui::{InstallerError, EXIT_USAGE};

use super::Outcome;
use crate::cx::Cx;
use crate::flows::uninstall::{self, UninstallArgs, What};

fn usage(msg: &str) -> InstallerError {
    InstallerError::new("E1870", msg)
        .with_fix("ciginstaller uninstall --help")
        .with_exit(EXIT_USAGE)
}

pub fn run(
    cx: &mut Cx,
    tool: Option<&str>,
    all: bool,
    purge: bool,
    keep_journal: bool,
    dry_run: bool,
) -> Result<Outcome, InstallerError> {
    let what = match (tool, all) {
        (Some(_), true) => return Err(usage("name a tool or say --all, not both")),
        (None, false) if !purge => return Err(usage("name a tool to uninstall, or say --all")),
        (Some(t), false) => What::Tool(t.to_string()),
        _ => What::All,
    };
    uninstall::run(
        cx,
        &UninstallArgs {
            what,
            purge,
            keep_journal,
            dry_run,
        },
    )
    .map(Outcome::Report)
}

pub fn remove(cx: &mut Cx, carton: &str, dry_run: bool) -> Result<Outcome, InstallerError> {
    uninstall::run(
        cx,
        &UninstallArgs {
            what: What::Carton(carton.to_string()),
            purge: false,
            keep_journal: false,
            dry_run,
        },
    )
    .map(Outcome::Report)
}
