// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `repair` (SPEC 12.7, #14): check each toolchain against its receipt and put back what is
//! missing or changed, from the store when the object is there and sound, else from a fresh
//! download that is admitted the way an install admits it (else `E1860`, with the home as it
//! was). Files the receipt does not list are removed. Then the shims, the index, the humidor
//! mirror and the PATH block the user agreed to are made right, all in one transaction.
//!
//! The spare of cig-orchestrator#64 is not a source in this build.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use cig_compat::Version;
use cigi_home::path::block_state;
use cigi_home::state::HistoryEntry;
use cigi_home::toolchain::{remove_extra, restore_file};
use cigi_home::{
    add_path_block, installed, now_rfc3339, path_block, read_receipt, remirror_humidor, shim_path,
    sync_shims, verify_toolchain, Damage, ObjectId, PathChange, Receipt, State,
};
use cigi_ui::{Icon, InstallerError, Report, EXIT_REFUSED};
use serde_json::json;

use super::health::{
    humidor_bad, index_state, installer_objects, missing_shims, recorded_shell,
    remove_committed_txn, shim_names, IndexState,
};
use super::install::fetch_builds;
use super::{cached_yanked, save_and_flip, settle};
use crate::cx::{Cx, ManifestMode};
use crate::errors::ie;

#[derive(Debug, Clone, Default)]
pub struct RepairArgs {
    /// Only this tool's toolchains.
    pub tool: Option<String>,
    /// Say what would be done; change nothing.
    pub dry_run: bool,
}

struct Bad {
    tool: String,
    ver: Version,
    receipt: Receipt,
    damage: Vec<Damage>,
}

impl Bad {
    fn label(&self) -> String {
        format!("{} {}", self.tool, self.ver)
    }
    /// Receipt files this toolchain must have put back: missing, changed, wrong mode, or a
    /// listed path that something else stands on.
    fn files_to_restore(&self) -> Vec<&cigi_home::ReceiptFile> {
        let paths: BTreeSet<&str> = self.damage.iter().filter_map(Damage::path).collect();
        self.receipt
            .files
            .iter()
            .filter(|f| paths.contains(f.path.as_str()))
            .collect()
    }
    fn extras(&self) -> Vec<&str> {
        self.damage
            .iter()
            .filter_map(|d| match d {
                Damage::Extra { path } => Some(path.as_str()),
                _ => None,
            })
            .collect()
    }
}

fn bad_download(tool: &str, ver: &Version, path: Option<&str>, id: &ObjectId) -> InstallerError {
    let what = path.map_or_else(|| id.sha256.clone(), str::to_string);
    InstallerError::new(
        "E1860",
        format!("{tool} {ver}: the download does not hold {what} as the receipt recorded it"),
    )
    .with_fix("ciginstaller repair")
    .with_exit(EXIT_REFUSED)
}

fn e1860(what: impl Into<String>, why: &InstallerError) -> InstallerError {
    InstallerError::new("E1860", what)
        .with_hint(format!("{} ({})", why.message, why.code))
        .with_fix("ciginstaller repair")
        .with_exit(EXIT_REFUSED)
}

pub fn run(cx: &mut Cx, a: &RepairArgs) -> Result<Report, InstallerError> {
    cx.lock_and_recover()?;
    let home = cx.home.clone();
    let state = cx.state.clone();

    if let Some(t) = &a.tool {
        if !installed(&home).iter().any(|(x, _)| x == t) {
            return Err(InstallerError::new(
                "E1863",
                format!("there is no installed toolchain of `{t}` to repair"),
            )
            .with_fix("ciginstaller list")
            .with_exit(EXIT_REFUSED));
        }
    }

    // 1. what is wrong (reads only)
    let mut bad: Vec<Bad> = Vec::new();
    for (tool, ver) in installed(&home) {
        if a.tool.as_ref().is_some_and(|t| *t != tool) {
            continue;
        }
        let damage = verify_toolchain(&home, &tool, &ver);
        if damage.is_empty() {
            continue;
        }
        if let Ok(receipt) = read_receipt(&home, &tool, &ver) {
            bad.push(Bad {
                tool,
                ver,
                receipt,
                damage,
            });
        }
    }
    let scoped = a.tool.is_some();
    let shims_missing = if scoped {
        Vec::new()
    } else {
        missing_shims(&home, &state).unwrap_or_default()
    };
    let index_wrong = !scoped
        && state.machine_carton.is_some()
        && index_state(&home, &state, cached_yanked(cx)) != IndexState::Agrees;
    let humidor = if scoped {
        Vec::new()
    } else {
        humidor_bad(&home)
    };
    // every cig that was mirrored and has a file to put back: its mirror links the damaged file
    let mut mirror: BTreeSet<Version> = humidor.iter().map(|(v, _, _)| v.clone()).collect();
    for b in &bad {
        if b.tool == "cig"
            && b.receipt.humidor_mirror.as_deref() == Some("mirrored")
            && !b.files_to_restore().is_empty()
        {
            mirror.insert(b.ver.clone());
        }
    }
    let shell = recorded_shell(&state);
    let path_missing = !scoped
        && shell.is_some_and(|s| matches!(block_state(s, &home), Ok(PathChange::NotPresent)));
    let path_edited =
        !scoped && shell.is_some_and(|s| matches!(block_state(s, &home), Ok(PathChange::Edited)));

    if bad.is_empty()
        && shims_missing.is_empty()
        && !index_wrong
        && humidor.is_empty()
        && !path_missing
    {
        let mut r = Report::done("nothing to repair: every toolchain matches its receipt, and the shims, index and humidor mirror are as they should be")
            .data(json!({ "restored": [], "removed": [], "changed": false }));
        if path_edited {
            r = r.detail("the PATH block was edited by hand and is left alone (E1862)");
        }
        return Ok(r);
    }

    // 2. which damaged files the store can give back, and which need a download
    let store = home.object_store();
    let mut need: BTreeMap<(String, Version), BTreeSet<ObjectId>> = BTreeMap::new();
    let mut from_store = 0usize;
    for b in &bad {
        for f in b.files_to_restore() {
            let id = ObjectId::new(f.sha256.clone(), f.exec);
            if store.verify(&id).is_ok() {
                from_store += 1;
            } else {
                need.entry((b.tool.clone(), b.ver.clone()))
                    .or_default()
                    .insert(id);
            }
        }
    }

    if a.dry_run {
        let mut r = Report::done(format!(
            "dry run: would restore {} file(s) from the store and download {} build(s) again",
            from_store,
            need.len()
        ));
        for b in &bad {
            for d in &b.damage {
                r = r.detail(format!("{}: {d:?}", b.label()));
            }
        }
        for s in &shims_missing {
            r = r.detail(format!("would relink the shim {s}"));
        }
        if index_wrong {
            r = r.detail("would regenerate the shim-index");
        }
        for v in &mirror {
            r = r.detail(format!("would mirror cig {v} into the humidor again"));
        }
        if path_missing {
            r = r.detail("would put the PATH block back");
        }
        return Ok(r
            .next("ciginstaller repair")
            .data(json!({ "dry_run": true, "from_store": from_store, "downloads": need.len() })));
    }

    // 3. downloads, before anything is written: a failure here leaves the home as it was
    let mut fetched = Vec::new();
    if !need.is_empty() {
        let list: Vec<String> = need.keys().map(|(t, v)| format!("{t} {v}")).collect();
        let m = cx
            .manifest(ManifestMode::Fresh)
            .map_err(|e| {
                e1860(
                    format!(
                        "{} is damaged, is not in the store, and cannot be downloaded again",
                        list.join(", ")
                    ),
                    &e,
                )
            })?
            .clone();
        let todo: Vec<(String, Version)> = need.keys().cloned().collect();
        fetched = fetch_builds(cx, &m, &todo).map_err(|e| {
            e1860(
                format!(
                    "{} is damaged, is not in the store, and cannot be downloaded again",
                    list.join(", ")
                ),
                &e,
            )
        })?;
    }
    // each missing object must be in the download, byte for byte what the receipt says
    let mut sources: BTreeMap<ObjectId, PathBuf> = BTreeMap::new();
    for ((tool, ver), ids) in &need {
        let f = fetched
            .iter()
            .find(|f| &f.tool == tool && &f.version == ver)
            .ok_or_else(|| {
                InstallerError::new("E1860", format!("{tool} {ver} was not downloaded"))
                    .with_exit(EXIT_REFUSED)
            })?;
        let receipt = bad
            .iter()
            .find(|b| &b.tool == tool && &b.ver == ver)
            .map(|b| &b.receipt);
        for id in ids {
            let path = receipt
                .and_then(|r| {
                    r.files
                        .iter()
                        .find(|x| x.sha256 == id.sha256 && x.exec == id.exec)
                })
                .map(|x| x.path.clone());
            let staged = path
                .as_ref()
                .and_then(|p| f.files.iter().find(|s| s.path == std::path::Path::new(p)));
            let ok = staged.is_some_and(|s| {
                cigi_source::digest::sha256_file(&s.source)
                    .map(|(h, _)| h == id.sha256)
                    .unwrap_or(false)
            });
            match (ok, staged) {
                (true, Some(s)) => {
                    sources.insert(id.clone(), s.source.clone());
                }
                _ => return Err(bad_download(tool, ver, path.as_deref(), id)),
            }
        }
    }

    // 4. the transaction
    let mut report_lines: Vec<String> = Vec::new();
    let mut restored: Vec<serde_json::Value> = Vec::new();
    let mut removed: Vec<serde_json::Value> = Vec::new();
    let mut txn = cx.begin("repair")?;
    let txn_id = txn.id().to_string();
    let mut new_state: State = state.clone();
    let applied = (|| -> Result<(), InstallerError> {
        // a store object that is damaged or gone is replaced by the downloaded one
        // (written in place of the old object, so a rollback gives the damaged bytes back as
        // they were; other toolchains' links to the old inode are made new below)
        for (id, src) in &sources {
            let p = store.object_path(id);
            let bytes = std::fs::read(src).map_err(|e| {
                InstallerError::new("E1860", format!("cannot read the download: {e}"))
                    .with_exit(EXIT_REFUSED)
            })?;
            if let Some(parent) = p.parent() {
                txn.mkdir_all(parent).map_err(ie)?;
            }
            txn.write(&p, &bytes, Some(id.mode())).map_err(ie)?;
        }
        for b in &bad {
            for rel in b.extras() {
                // a listed path with something else on it is replaced below, not just removed
                remove_extra(&mut txn, &home, &b.tool, &b.ver, rel).map_err(ie)?;
                if !b.receipt.files.iter().any(|f| f.path == rel) {
                    removed
                        .push(json!({ "tool": b.tool, "version": b.ver.to_string(), "path": rel }));
                    report_lines.push(format!(
                        "removed {rel} from {} (the receipt does not list it)",
                        b.label()
                    ));
                }
            }
            for f in b.files_to_restore() {
                let id = ObjectId::new(f.sha256.clone(), f.exec);
                let how = if sources.contains_key(&id) {
                    "from a fresh download"
                } else {
                    "from the store"
                };
                restore_file(&mut txn, &home, &b.tool, &b.ver, f).map_err(ie)?;
                restored.push(json!({ "tool": b.tool, "version": b.ver.to_string(), "path": f.path, "from": how }));
                report_lines.push(format!("restored {} of {} {how}", f.path, b.label()));
            }
        }
        // shims and the installer's own links: every run of a whole repair, and a run for the
        // installer's own toolchain
        let ci_bad = bad.iter().any(|b| b.tool == "ciginstaller");
        if !scoped || ci_bad {
            if let Some((exe_obj, shim_obj)) = installer_objects(&home, &new_state) {
                let names = shim_names(&home, &new_state);
                let refs: Vec<&str> = names.iter().map(String::as_str).collect();
                sync_shims(&mut txn, &home, &refs, &shim_obj).map_err(ie)?;
                for n in ["ciginstaller", "cigi"] {
                    txn.link(&store.object_path(&exe_obj), &shim_path(&home, n))
                        .map_err(ie)?;
                }
                for s in &shims_missing {
                    report_lines.push(format!("relinked the shim {s}"));
                }
            }
        }
        // the humidor mirror: what was out of step, and every cig whose binary was just put back
        // (the mirror is a link of the old, damaged file)
        for ver in &mirror {
            if let Some(exe) = super::tool_exe(&home, "cig", ver) {
                remirror_humidor(&mut txn, &home, ver, &exe).map_err(ie)?;
                report_lines.push(format!("mirrored cig {ver} into the humidor again"));
            }
        }
        if !scoped && path_missing {
            if let Some(s) = shell {
                let (profile, _) = path_block(s, &home);
                if add_path_block(&mut txn, s, &home).map_err(ie)? == PathChange::Added {
                    report_lines.push(format!("put the PATH block back in {}", profile.display()));
                }
            }
        }
        if index_wrong {
            report_lines.push("regenerated the shim-index from state.json".to_string());
        }
        let now = now_rfc3339();
        new_state.push_history(HistoryEntry {
            id: txn_id.clone(),
            at: now.clone(),
            what: "repair".into(),
            from: None,
            to: None,
            txn: txn_id.clone(),
            ok: true,
        });
        save_and_flip(&mut txn, &home, &new_state, cached_yanked(cx))
    })();
    settle(txn, applied)?;
    drop(fetched);

    remove_committed_txn(&home, &txn_id);
    store.sweep_tmp();
    cx.state = State::load(&home).map_err(ie)?;

    let mut r = Report::new(
        Icon::Done,
        format!(
            "repaired: {} file(s) restored, {} removed",
            restored.len(),
            removed.len()
        ),
    );
    for l in &report_lines {
        r = r.detail(l.clone());
    }
    if path_edited {
        r = r.detail("the PATH block was edited by hand and is left alone (E1862)");
    }
    Ok(r.data(json!({
        "changed": true,
        "restored": restored,
        "removed": removed,
    })))
}
