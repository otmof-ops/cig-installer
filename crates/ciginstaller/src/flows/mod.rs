// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! The flows (SPEC 2.6 and 12). Each is `fn run(cx, args) -> Result<Report, InstallerError>`.

pub mod bench;
pub mod bug;
pub mod bundle;
pub mod gc;
pub mod install;
pub mod mirror;
pub mod programs;
// ---- the update lifecycle ----
pub mod notice;
pub mod rollback;
pub mod switch;
pub mod trial;
pub mod tripwire;
pub mod update;
// ---- end of the update lifecycle block ----
pub mod self_update;
pub mod terms;
pub mod use_carton;
// ---- health: doctor, repair, uninstall ----
pub mod doctor;
pub mod family;
pub mod health;
pub mod repair;
pub mod report;
pub mod uninstall;
// ---- end of the health block ----

use std::path::PathBuf;

use cig_compat::{Manifest, Version, Yankable};
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
    let target = r.shim_target(tool);
    // the path comes from a file on disk: it must stay inside its toolchain
    if !cigi_home::toolchain::safe_rel(&target) {
        return None;
    }
    let dir = home.toolchain(tool, v);
    let mut p = dir.clone();
    for part in target.split('/') {
        p.push(part);
    }
    p.starts_with(&dir).then_some(p)
}

/// The last steps of every flow that changes what runs: `state.json`, then the `shim-index` flip.
/// The caller commits (or rolls back on an error).
///
/// `yanked` names the cartons the manifest at hand (or the cached one) yanks: the index never
/// names one for a `0.1` or `0` pin, so the shim's fast path and the resolver agree (SPEC 8).
pub fn save_and_flip(
    txn: &mut Txn,
    home: &Home,
    state: &State,
    yanked: Vec<String>,
) -> Result<(), InstallerError> {
    // the folder the home lives in is recorded once, so a moved home can say where it came from
    let mut state = state.clone();
    state.record_home(&home.root().to_string_lossy(), false);
    let state = &state;
    state.save_in(txn).map_err(ie)?;
    let index = ShimIndex::from_state(home, state).with_yanked(yanked);
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

/// The cartons a manifest yanks, as version strings.
pub fn yanked_in(m: &Manifest) -> Vec<String> {
    m.yanked
        .iter()
        .filter_map(|y| match &y.what {
            Yankable::Carton(v) => Some(v.to_string()),
            _ => None,
        })
        .collect()
}

/// The yanked cartons of the cached manifest (none when there is no cache).
pub fn cached_yanked(cx: &crate::cx::Cx) -> Vec<String> {
    cx.peek_manifest()
        .map(|m| yanked_in(&m))
        .unwrap_or_default()
}
