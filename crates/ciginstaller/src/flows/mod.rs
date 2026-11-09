// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0
//! The flows (SPEC 2.6 and 12). Each is `fn run(cx, args) -> Result<Report, InstallerError>`.

pub mod install;
pub mod use_carton;

use std::path::PathBuf;

use cig_compat::Version;
use cigi_home::{read_receipt, verify_toolchain, FlipIntent, Home, Journal, ShimIndex, State, Txn};
use cigi_ui::InstallerError;

use crate::errors::ie;

/// Is this exact build installed and intact (a receipt that its files match)?
pub fn present(home: &Home, tool: &str, v: &Version) -> bool {
    read_receipt(home, tool, v).is_ok() && verify_toolchain(home, tool, v).is_empty()
}

/// The file a shim should run for a tool build, from its receipt.
pub fn tool_exe(home: &Home, tool: &str, v: &Version) -> Option<PathBuf> {
    let r = read_receipt(home, tool, v).ok()?;
    let mut p = home.toolchain(tool, v);
    for part in r.shim_target(tool).split('/') {
        p.push(part);
    }
    Some(p)
}

/// The last steps of every flow that changes what runs: `state.json`, then the `shim-index` flip.
/// The caller commits (or rolls back on an error).
pub fn save_and_flip(txn: &mut Txn, home: &Home, state: &State) -> Result<(), InstallerError> {
    state.save_in(txn).map_err(ie)?;
    let index = ShimIndex::from_state(home, state);
    index.check().map_err(ie)?;
    txn.flip(FlipIntent {
        index: index.render(),
    })
    .map_err(ie)
}

/// Finish a transaction: commit on success; on failure roll back and say so if the undo was short.
pub fn settle<T>(txn: Txn, result: Result<T, InstallerError>) -> Result<T, InstallerError> {
    match result {
        Ok(v) => {
            txn.commit().map_err(ie)?;
            Ok(v)
        }
        Err(e) => {
            match txn.rollback() {
                Ok(rep) if rep.is_complete() => Err(e),
                Ok(rep) => Err(InstallerError::new(
                    "E1833",
                    format!(
                        "{} (and the undo is incomplete: {} step(s) failed; the next command finishes it)",
                        e.message,
                        rep.failed.len()
                    ),
                )
                .with_fix("ciginstaller doctor")),
                Err(_) => Err(e),
            }
        }
    }
}
