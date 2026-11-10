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

use cig_compat::Version;
use cigi_home::path::block_state;
use cigi_home::state::HistoryEntry;
use cigi_home::toolchain::{remove_extra, restore_file};
use cigi_home::{
    add_path_block, installed, now_rfc3339, path_block, read_receipt, recorded_registry,
    recorded_shells, registry_path_add, registry_path_has, remirror_humidor, shim_path, sync_shims,
    verify_toolchain, Damage, ObjectId, PathChange, Receipt, Shell, State,
};
use cigi_ui::{Icon, InstallerError, Report, EXIT_REFUSED};
use serde_json::json;

use super::health::{
    builds, humidor_bad, index_state, installer_objects, missing_shims, pending_txns,
    remove_committed_txn, shim_names, IndexState,
};
use super::install::{fetch_builds, materialize_fetched};
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

/// E1860 for a download that could not be had. The fix names the tool: it is run again, online,
/// when the manifest and the artifact can be reached.
fn e1860(what: impl Into<String>, why: &InstallerError, tool: &str) -> InstallerError {
    InstallerError::new("E1860", what)
        .with_hint(format!(
            "{} ({}); connect, then run it again",
            why.message, why.code
        ))
        .with_fix(format!("ciginstaller repair --tool {tool}"))
        .with_exit(EXIT_REFUSED)
}

/// The bytes of a staged download: read once, from a plain file (never a link), and hashed as
/// read. What is returned is what was hashed, and is what gets written.
fn read_staged(path: &std::path::Path, want_sha256: &str) -> Result<Vec<u8>, String> {
    let md = std::fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !md.file_type().is_file() {
        return Err(format!(
            "{} is not a plain file (a link or a directory stands there)",
            path.display()
        ));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if cigi_source::digest::sha256_hex(&bytes) != want_sha256 {
        return Err(format!("{} does not hash to {want_sha256}", path.display()));
    }
    Ok(bytes)
}

pub fn run(cx: &mut Cx, a: &RepairArgs) -> Result<Report, InstallerError> {
    // a dry run reads and takes no lock: no layout is made, nothing is recovered
    if !a.dry_run {
        cx.lock_and_recover()?;
    }
    let home = cx.home.clone();
    let state = cx.state.clone();
    let interrupted = pending_txns(&home);

    if let Some(t) = &a.tool {
        let named = builds(&state).iter().any(|(x, _)| x == t);
        if !installed(&home).iter().any(|(x, _)| x == t) && !named {
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
    // toolchains the state lists that have no readable receipt (or no directory at all): rebuilt
    // from the manifest's artifact, with a fresh receipt
    let have: BTreeSet<(String, String)> = installed(&home)
        .into_iter()
        .map(|(t, v)| (t, v.to_string()))
        .collect();
    let mut rebuild: Vec<(String, Version, Version)> = Vec::new();
    for (t, vs) in builds(&state) {
        if a.tool.as_ref().is_some_and(|x| *x != t) {
            continue;
        }
        let Ok(ver) = Version::parse(&vs) else {
            continue;
        };
        if have.contains(&(t.clone(), ver.to_string())) {
            continue;
        }
        let carton = state
            .cartons
            .iter()
            .find(|(_, r)| r.tools.get(&t) == Some(&vs))
            .map(|(c, _)| c.clone())
            .or_else(|| {
                state
                    .sticks
                    .iter()
                    .find(|s| s.tool == t && s.version == vs)
                    .map(|s| s.from_carton.clone())
            })
            .or_else(|| state.machine_carton.clone());
        let Some(cv) = carton.and_then(|c| Version::parse(&c).ok()) else {
            continue;
        };
        rebuild.push((t, ver, cv));
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
    let shells = recorded_shells(&state);
    let blocks_missing: Vec<Shell> = if scoped {
        Vec::new()
    } else {
        shells
            .iter()
            .copied()
            .filter(|s| matches!(block_state(*s, &home), Ok(PathChange::NotPresent)))
            .collect()
    };
    // the registry entry the install recorded, gone from the user's PATH (re-added after the
    // commit, as the install did)
    let registry_missing = if scoped {
        None
    } else {
        recorded_registry(&state)
            .filter(|r| matches!(registry_path_has(&r.entry), Ok(false)))
            .map(|r| r.entry)
    };
    let path_missing = !blocks_missing.is_empty() || registry_missing.is_some();
    let path_edited = !scoped
        && shells
            .iter()
            .any(|s| matches!(block_state(*s, &home), Ok(PathChange::Edited)));

    if bad.is_empty()
        && rebuild.is_empty()
        && shims_missing.is_empty()
        && !index_wrong
        && humidor.is_empty()
        && !path_missing
    {
        let mut r = Report::done("nothing to repair: every toolchain matches its receipt, and the shims, index and humidor mirror are as they should be")
            .data(json!({
                "restored": [],
                "removed": [],
                "changed": false,
                "dry_run": a.dry_run,
                "interrupted": interrupted,
            }));
        if path_edited {
            r = r.detail("the PATH block was edited by hand and is left alone (E1862)");
        }
        for t in &interrupted {
            r = r.detail(format!("an interrupted transaction is waiting: {t}"));
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
        for (t, v, _) in &rebuild {
            r = r.detail(format!(
                "would build {t} {v} again from the manifest's artifact (no readable receipt)"
            ));
        }
        for t in &interrupted {
            r = r.detail(format!(
                "an interrupted transaction is waiting: {t}; a real run settles it first"
            ));
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
        if !blocks_missing.is_empty() {
            r = r.detail("would put the PATH block back");
        }
        if let Some(e) = &registry_missing {
            r = r.detail(format!(
                "would put {} back on {}",
                e.display(),
                cigi_home::path::REGISTRY_PATH
            ));
        }
        return Ok(r
            .next("ciginstaller repair")
            .data(json!({
                "dry_run": true,
                "from_store": from_store,
                "downloads": need.len(),
                "rebuild": rebuild.iter().map(|(t, v, _)| json!({ "tool": t, "version": v.to_string() })).collect::<Vec<_>>(),
                "interrupted": interrupted,
            })));
    }

    // 3. downloads, before anything is written: a failure here leaves the home as it was
    let mut fetched = Vec::new();
    let mut manifest = None;
    let mut todo: Vec<(String, Version)> = need.keys().cloned().collect();
    for (t, v, _) in &rebuild {
        if !todo.contains(&(t.clone(), v.clone())) {
            todo.push((t.clone(), v.clone()));
        }
    }
    if !todo.is_empty() {
        let list: Vec<String> = todo.iter().map(|(t, v)| format!("{t} {v}")).collect();
        let first = todo[0].0.clone();
        let what = format!(
            "{} is damaged or has no receipt, is not in the store, and cannot be downloaded again",
            list.join(", ")
        );
        let m = cx
            .manifest(ManifestMode::Fresh)
            .map_err(|e| e1860(what.clone(), &e, &first))?
            .clone();
        fetched = fetch_builds(cx, &m, &todo).map_err(|e| e1860(what.clone(), &e, &first))?;
        manifest = Some(m);
    }
    // each missing object must be in the download, byte for byte what the receipt says; the
    // bytes are read once, hashed, and are what is written
    let mut sources: BTreeMap<ObjectId, Vec<u8>> = BTreeMap::new();
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
            match staged.map(|s| read_staged(&s.source, &id.sha256)) {
                Some(Ok(bytes)) => {
                    sources.insert(id.clone(), bytes);
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
        for (id, bytes) in &sources {
            let p = store.object_path(id);
            if let Some(parent) = p.parent() {
                txn.mkdir_all(parent).map_err(ie)?;
            }
            txn.write(&p, bytes, Some(id.mode())).map_err(ie)?;
        }
        // toolchains without a readable receipt: built again, admitted as an install admits them
        if let Some(m) = &manifest {
            for (t, v, cv) in &rebuild {
                if let Some(f) = fetched.iter().find(|f| &f.tool == t && &f.version == v) {
                    materialize_fetched(&mut txn, &home, m, cv, std::slice::from_ref(f))?;
                    restored.push(json!({ "tool": t, "version": v.to_string(), "path": "(the whole toolchain)", "from": "a fresh download" }));
                    report_lines.push(format!(
                        "built {t} {v} again from the manifest's artifact, with a new receipt"
                    ));
                }
            }
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
        let ci_bad = bad.iter().any(|b| b.tool == "ciginstaller")
            || rebuild.iter().any(|(t, _, _)| t == "ciginstaller");
        if !scoped || ci_bad {
            if let Some((exe_obj, shim_obj)) = installer_objects(&home, &new_state) {
                let names = shim_names(&home, &new_state);
                let refs: Vec<&str> = names.iter().map(String::as_str).collect();
                sync_shims(&mut txn, &home, &refs, &shim_obj).map_err(ie)?;
                for n in super::switch::installer_names() {
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
        for s in &blocks_missing {
            let (profile, _) = path_block(*s, &home);
            if add_path_block(&mut txn, *s, &home).map_err(ie)? == PathChange::Added {
                report_lines.push(format!("put the PATH block back in {}", profile.display()));
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
    if let Some(entry) = &registry_missing {
        match registry_path_add(entry) {
            Ok(_) => report_lines.push(format!(
                "put {} back on {}",
                entry.display(),
                cigi_home::path::REGISTRY_PATH
            )),
            Err(e) => report_lines.push(format!(
                "could not put {} back on {} (E1845): {e}",
                entry.display(),
                cigi_home::path::REGISTRY_PATH
            )),
        }
    }

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

// Unix only: proves a staged download that is a symlink is never read.
#[cfg(all(test, unix))]
mod tests {
    use super::read_staged;
    use std::os::unix::fs::symlink;

    #[test]
    fn a_staged_download_is_read_once_hashed_and_never_a_link() {
        let t = tempfile::tempdir().unwrap();
        let good = t.path().join("good");
        std::fs::write(&good, b"the bytes").unwrap();
        let sha = cigi_source::digest::sha256_hex(b"the bytes");
        // what was hashed is what comes back
        assert_eq!(read_staged(&good, &sha).unwrap(), b"the bytes");
        // other bytes do not match
        let e = read_staged(&good, &cigi_source::digest::sha256_hex(b"other")).unwrap_err();
        assert!(e.contains("does not hash"), "{e}");
        // a link is refused even when what it points at has the right bytes
        let link = t.path().join("link");
        symlink(&good, &link).unwrap();
        let e = read_staged(&link, &sha).unwrap_err();
        assert!(e.contains("not a plain file"), "{e}");
        // so is a directory, and a path that is not there
        assert!(read_staged(t.path(), &sha).is_err());
        assert!(read_staged(&t.path().join("none"), &sha).is_err());
    }
}
