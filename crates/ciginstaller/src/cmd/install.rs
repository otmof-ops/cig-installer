// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0
//! `install [carton | pack <name> | pack --pick a,b | stick <tool>]`.

use cigi_source::{MirrorSource, TrustLevel};
use cigi_ui::{InstallerError, EXIT_REFUSED, EXIT_USAGE};

use super::Outcome;
use crate::cli::{InstallCmd, InstallWhat};
use crate::cx::Cx;
use crate::errors::ie;
use crate::flows::install::{self, InstallArgs, What};

pub fn run(cx: &mut Cx, c: &InstallCmd) -> Result<Outcome, InstallerError> {
    let what = match &c.what {
        None => {
            return Err(InstallerError::new(
                "E1871",
                "say what to install: a carton, a pack or a stick",
            )
            .with_fix("ciginstaller install carton")
            .with_exit(EXIT_USAGE))
        }
        Some(InstallWhat::Carton) => What::Carton,
        Some(InstallWhat::Stick { tool }) => What::Stick(tool.clone()),
        Some(InstallWhat::Pack { name, pick }) => match (name, pick.is_empty()) {
            (Some(n), true) => What::Pack(n.clone()),
            (None, false) => What::Pick(pick.clone()),
            _ => {
                return Err(InstallerError::new(
                    "E1870",
                    "`install pack` takes a pack name or --pick a,b, not both and not neither",
                )
                .with_fix("ciginstaller install pack dev")
                .with_exit(EXIT_USAGE))
            }
        },
    };
    if let Some(ch) = &c.channel {
        cx.cfg
            .set_for_run("channel", ch)
            .map_err(|_| no_such_channel(ch))?;
    }
    if let Some(dir) = &c.from {
        let abs = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.clone());
        cx.source = Box::new(MirrorSource::parse(&abs.to_string_lossy()).map_err(ie)?);
    }
    if let Some(level) = &c.require {
        let l = TrustLevel::parse(level).ok_or_else(|| {
            InstallerError::new("E1870", format!("`{level}` is not a trust level"))
                .with_fix("ciginstaller install carton --require checksummed")
                .with_exit(EXIT_USAGE)
        })?;
        cx.require_trust(l);
    }
    let args = InstallArgs {
        what,
        carton: c.carton.clone(),
        no_path: c.no_path,
        shell: c.shell.clone(),
        allow_yanked: c.allow_yanked,
        dry_run: c.dry_run,
        reason: None,
    };
    install::run(cx, &args).map(Outcome::Report)
}

/// E1812: a channel name that is not one of the three.
pub fn no_such_channel(name: &str) -> InstallerError {
    InstallerError::new("E1812", format!("there is no channel `{name}`"))
        .with_hint("the channels are stable, beta and nightly")
        .with_fix("ciginstaller channel stable")
        .with_exit(EXIT_REFUSED)
}
