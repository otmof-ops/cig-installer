// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! What `update` and `rollback` share: the shims a state calls for, the record that lets a
//! rollback undo the update that preceded it, and the repair of a toolchain before a switch to it.

use std::collections::BTreeSet;

use cig_compat::Version;
use cigi_home::state::Previous;
use cigi_home::toolchain::{installed, remove_extra, restore_file, Damage};
use cigi_home::{
    read_receipt, remove_shims, sync_shims, verify_toolchain, Home, ObjectId, State, Txn,
};
use cigi_ui::{InstallerError, EXIT_REFUSED};
use serde_json::{json, Value};

use crate::errors::{bug, ie};

/// Where `state.json` keeps what `rollback` needs to undo the update that made the machine carton:
/// `{ "added": <carton the update installed>, "from": <the carton it left>, "previous": <what
/// `state.previous` was before it> }`. Unknown keys survive in `state.extra`, so an older
/// installer reading this file keeps it.
pub const UNDO_KEY: &str = "update_undo";

/// What an update left behind that `rollback` may undo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateUndo {
    pub added: String,
    pub from: String,
    pub previous: Previous,
    /// The history id of the update that wrote this record.
    pub txn: String,
}

impl UpdateUndo {
    pub fn to_value(&self) -> Value {
        json!({
            "added": self.added,
            "from": self.from,
            "txn": self.txn,
            "previous": {
                "machine_carton": self.previous.machine_carton,
                "kept_until": self.previous.kept_until,
            },
        })
    }

    pub fn read(state: &State) -> Option<UpdateUndo> {
        let v = state.extra.get(UNDO_KEY)?;
        let text = |x: &Value| x.as_str().map(str::to_string);
        let prev = v.get("previous")?;
        Some(UpdateUndo {
            added: text(v.get("added")?)?,
            from: text(v.get("from")?)?,
            txn: text(v.get("txn")?)?,
            previous: Previous {
                machine_carton: prev.get("machine_carton").and_then(text),
                kept_until: prev.get("kept_until").and_then(text),
            },
        })
    }

    /// Does it still describe the machine: the update is the newest history entry, the machine is
    /// on the carton it added, and the one it left is the previous? (Any other switch since makes
    /// it stale, and it is then ignored.)
    pub fn applies(&self, state: &State) -> bool {
        // a rollback right after its update: the update is the newest thing that happened
        state
            .history
            .last()
            .is_some_and(|h| h.id == self.txn && h.what == "update")
            && state.machine_carton.as_deref() == Some(self.added.as_str())
            && state.previous.machine_carton.as_deref() == Some(self.from.as_str())
            && state.cartons.contains_key(&self.added)
    }
}

/// The shim names the builds in `state` serve (only the names the shim binary answers to).
pub fn shim_names(home: &Home, state: &State) -> BTreeSet<String> {
    let mut builds: Vec<(String, String)> = Vec::new();
    for r in state.cartons.values() {
        builds.extend(r.tools.iter().map(|(t, v)| (t.clone(), v.clone())));
    }
    builds.extend(
        state
            .sticks
            .iter()
            .map(|s| (s.tool.clone(), s.version.clone())),
    );
    let mut names = BTreeSet::new();
    for (t, v) in &builds {
        if let Ok(v) = Version::parse(v) {
            if let Ok(r) = read_receipt(home, t, &v) {
                names.extend(
                    r.shims
                        .iter()
                        .map(|s| s.name.clone())
                        .filter(|n| cigi_shim::TOOLS.contains(&n.as_str())),
                );
            }
        }
    }
    names
}

/// The store object every shim in `bin/` is a link of: `bin/cigi-shim` of the installer's toolchain.
pub fn shim_object(home: &Home, state: &State) -> Option<ObjectId> {
    let of = |v: &Version| -> Option<ObjectId> {
        let r = read_receipt(home, "ciginstaller", v).ok()?;
        let f = r.files.iter().find(|f| f.path == "bin/cigi-shim")?;
        Some(ObjectId::new(f.sha256.clone(), f.exec))
    };
    Version::parse(&state.installer.version)
        .ok()
        .and_then(|v| of(&v))
        .or_else(|| {
            let mut all: Vec<Version> = installed(home)
                .into_iter()
                .filter(|(t, _)| t == "ciginstaller")
                .map(|(_, v)| v)
                .collect();
            all.sort();
            all.iter().rev().find_map(of)
        })
}

/// Make `bin/` serve the names `after` calls for and no others that `before` served. Does nothing
/// when the two sets are the same.
pub fn resync_shims(
    txn: &mut Txn,
    home: &Home,
    before: &State,
    after: &State,
) -> Result<(), InstallerError> {
    let (old, new) = (shim_names(home, before), shim_names(home, after));
    if old == new {
        return Ok(());
    }
    let obj = shim_object(home, after).ok_or_else(|| {
        bug("the installer's own toolchain has no cigi-shim to link the shims to")
    })?;
    let add: Vec<&str> = new.iter().map(String::as_str).collect();
    sync_shims(txn, home, &add, &obj).map_err(ie)?;
    let gone: Vec<&str> = old.difference(&new).map(String::as_str).collect();
    remove_shims(txn, home, &gone).map_err(ie)
}

/// Before a switch to `tool` `v`: it must verify against its receipt. A damaged one is repaired
/// from the store inside `txn`; one that cannot be (no receipt, a store object missing or
/// damaged) refuses the switch with `E1831`.
pub fn verify_or_repair(
    txn: &mut Txn,
    home: &Home,
    tool: &str,
    v: &Version,
) -> Result<(), InstallerError> {
    let damage = verify_toolchain(home, tool, v);
    if damage.is_empty() {
        return Ok(());
    }
    let refuse = |why: String| {
        InstallerError::new(
            "E1831",
            format!("{tool} {v} differs from its receipt: {why}"),
        )
        .with_fix("ciginstaller repair")
        .with_exit(EXIT_REFUSED)
    };
    let receipt = read_receipt(home, tool, v).map_err(|e| refuse(e.to_string()))?;
    for d in &damage {
        match d {
            Damage::NoReceipt => return Err(refuse("it has no receipt".to_string())),
            Damage::Extra { path } => {
                remove_extra(txn, home, tool, v, path).map_err(|e| refuse(e.to_string()))?
            }
            Damage::Missing { path } | Damage::Changed { path, .. } | Damage::Mode { path } => {
                let f = receipt
                    .files
                    .iter()
                    .find(|f| &f.path == path)
                    .ok_or_else(|| refuse(format!("{path} is not in the receipt")))?;
                restore_file(txn, home, tool, v, f).map_err(|e| {
                    refuse(format!("{path} cannot be restored from the store: {e}"))
                })?;
            }
        }
    }
    let left = verify_toolchain(home, tool, v);
    if left.is_empty() {
        Ok(())
    } else {
        Err(refuse(format!(
            "{} file(s) are still wrong after the repair",
            left.len()
        )))
    }
}
