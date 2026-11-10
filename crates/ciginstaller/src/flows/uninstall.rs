// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `uninstall <tool>`, `uninstall --all` and `remove <carton>` (SPEC 12.8, #14).
//!
//! `uninstall <tool>` and `remove <carton>` are ordinary transactions that end in the `shim-index`
//! flip; the toolchains they stop using are deleted after the commit, the way `gc` does it.
//!
//! `uninstall --all` is one transaction without a flip, so a crash anywhere before its last step
//! rolls back to the whole home. Its last step is the rename of `installer/` to a sibling of the
//! home (`cigi_home::uninstall`): that rename is the commit point, and after it there is nothing
//! to recover. It leaves what is the user's: `runs/`, `tin/`, `config`, `locks/`, `inbox/`,
//! `ashtray/`, `cache/`, `cigcompile/`. The guard's hooks, daemon, eBPF programs, WDAC policy, DNS
//! filter and scheduled jobs do not exist in this build, so there is nothing of them to remove.

use std::collections::{BTreeSet, HashSet};
use std::path::PathBuf;

use cig_compat::Version;
use cigi_home::path::block_state;
use cigi_home::state::HistoryEntry;
use cigi_home::toolchain::referenced_objects;
use cigi_home::uninstall::{discard_aside, leftover_asides, move_installer_aside};
use cigi_home::{
    installed, now_rfc3339, path_block, read_receipt, remove_all_humidor_mirrors,
    remove_humidor_mirror, remove_path_block, remove_shims, shim_path, Home, Journal, ObjectId,
    PathChange, Shell, State, Txn,
};
use cigi_ui::{Icon, InstallerError, Question, Report, EXIT_REFUSED, EXIT_USAGE};
use serde_json::json;

use super::gc::{finish_pending, record_pending};
use super::health::{remove_committed_txn, shim_names, size_text};
use super::{cached_yanked, save_and_flip, settle};
use crate::cx::Cx;
use crate::errors::ie;

#[derive(Debug, Clone)]
pub enum What {
    /// All of one tool's builds, its shim, its place in the cartons.
    Tool(String),
    /// One installed carton, and the builds nothing else needs.
    Carton(String),
    /// Everything the installer put on the machine.
    All,
}

#[derive(Debug, Clone)]
pub struct UninstallArgs {
    pub what: What,
    /// Refused: user data is never removed by this build.
    pub purge: bool,
    /// Keep the journal of `--all` in `<home>-uninstall-<id>` instead of deleting it.
    pub keep_journal: bool,
    pub dry_run: bool,
}

fn refused(what: impl Into<String>, fix: &str) -> InstallerError {
    InstallerError::new("E1863", what)
        .with_fix(fix)
        .with_exit(EXIT_REFUSED)
}

pub fn run(cx: &mut Cx, a: &UninstallArgs) -> Result<Report, InstallerError> {
    if a.purge {
        return Err(InstallerError::new(
            "E1861",
            "--purge removes your data, and this build does not do that",
        )
        .with_hint("run records, the tin, config and backups stay where they are")
        .with_fix("ciginstaller uninstall --all")
        .with_exit(EXIT_REFUSED));
    }
    match &a.what {
        What::All => all(cx, a),
        What::Tool(t) => tool(cx, t, a.dry_run),
        What::Carton(c) => carton(cx, c, a.dry_run),
    }
}

// ---------------------------------------------------------------- one tool, one carton

/// What a change that stops using some builds needs: the state without them, and which
/// toolchains go.
fn remove_builds(
    cx: &mut Cx,
    label: &str,
    mutate: impl FnOnce(&mut State),
    gone: Vec<(String, Version)>,
) -> Result<Vec<String>, InstallerError> {
    let home = cx.home.clone();
    let before_names = shim_names(&home, &cx.state);
    let mut state = cx.state.clone();
    mutate(&mut state);
    let after_names = shim_names(&home, &state);
    let dead: Vec<String> = before_names.difference(&after_names).cloned().collect();
    let mut removed: Vec<String> = Vec::new();
    let mut txn = cx.begin(label)?;
    let txn_id = txn.id().to_string();
    let applied = (|| -> Result<(), InstallerError> {
        for (t, v) in &gone {
            if t == "cig" && remove_humidor_mirror(&mut txn, &home, v).map_err(ie)? {
                removed.push(home.humidor().join(v.to_string()).display().to_string());
            }
        }
        let refs: Vec<&str> = dead.iter().map(String::as_str).collect();
        remove_shims(&mut txn, &home, &refs).map_err(ie)?;
        for d in &dead {
            removed.push(shim_path(&home, d).display().to_string());
        }
        // the index flip below stops naming these; the directories go after the commit
        record_pending(&mut state, &gone);
        let now = now_rfc3339();
        state.push_history(HistoryEntry {
            id: txn_id.clone(),
            at: now.clone(),
            what: label.to_string(),
            from: None,
            to: None,
            txn: txn_id.clone(),
            ok: true,
        });
        save_and_flip(&mut txn, &home, &state, cached_yanked(cx))
    })();
    settle(txn, applied)?;
    for (t, v) in &gone {
        removed.push(home.toolchain(t, v).display().to_string());
    }
    finish_pending(cx, &gone)?;
    free_store(&home);
    remove_committed_txn(&home, &txn_id);
    cx.state = State::load(&home).map_err(ie)?;
    Ok(removed)
}

/// After the commit: objects nothing references any more.
fn free_store(home: &Home) -> (usize, u64) {
    let store = home.object_store();
    let keep: HashSet<ObjectId> = referenced_objects(home).into_keys().collect();
    let (mut n, mut bytes) = (0, 0);
    for (id, b) in store.unreferenced(&keep) {
        if store.remove(&id).is_ok() {
            n += 1;
            bytes += b;
        }
    }
    store.sweep_tmp();
    (n, bytes)
}

fn tool(cx: &mut Cx, t: &str, dry_run: bool) -> Result<Report, InstallerError> {
    if t == "ciginstaller" || t == "cigi" {
        return Err(refused(
            "ciginstaller is not removed one tool at a time",
            "ciginstaller uninstall --all",
        ));
    }
    cx.lock_and_recover()?;
    let home = cx.home.clone();
    let gone: Vec<(String, Version)> = installed(&home)
        .into_iter()
        .filter(|(x, _)| x == t)
        .collect();
    let named = super::health::builds(&cx.state).iter().any(|(x, _)| x == t);
    if gone.is_empty() && !named {
        return Err(refused(
            format!("`{t}` is not installed"),
            "ciginstaller list",
        ));
    }
    let versions: Vec<String> = gone.iter().map(|(_, v)| v.to_string()).collect();
    if dry_run {
        let mut r = Report::done(format!(
            "dry run: would remove {t} {}",
            if versions.is_empty() {
                "(its entries in state.json)".to_string()
            } else {
                versions.join(", ")
            }
        ));
        for (x, v) in &gone {
            r = r.detail(format!("would remove {}", home.toolchain(x, v).display()));
        }
        return Ok(r
            .next(format!("ciginstaller uninstall {t}"))
            .data(json!({ "dry_run": true, "tool": t, "versions": versions })));
    }
    let tool_name = t.to_string();
    let removed = remove_builds(
        cx,
        &format!("uninstall {t}"),
        |s| {
            for rec in s.cartons.values_mut() {
                if rec.tools.remove(&tool_name).is_some() {
                    rec.partial = true;
                }
            }
            s.sticks.retain(|x| x.tool != tool_name);
        },
        gone,
    )?;
    let mut r = Report::done(format!(
        "{t} is uninstalled: {} version(s) removed; the cartons that had it are marked partial",
        versions.len()
    ));
    for p in &removed {
        r = r.detail(format!("removed {p}"));
    }
    Ok(r.next("ciginstaller list").data(json!({
        "tool": t, "versions": versions, "removed": removed,
    })))
}

fn carton(cx: &mut Cx, c: &str, dry_run: bool) -> Result<Report, InstallerError> {
    cx.lock_and_recover()?;
    let home = cx.home.clone();
    let name = Version::parse(c).map_or_else(|_| c.to_string(), |v| v.to_string());
    let Some(rec) = cx.state.cartons.get(&name).cloned() else {
        return Err(refused(
            format!("carton {name} is not installed"),
            "ciginstaller list",
        ));
    };
    if cx.state.machine_carton.as_deref() == Some(name.as_str()) {
        return Err(refused(
            format!("carton {name} is the one this machine is on"),
            "ciginstaller use <another carton>",
        ));
    }
    if cx.state.previous.machine_carton.as_deref() == Some(name.as_str()) {
        return Err(refused(
            format!("carton {name} is the one `rollback` returns to"),
            "ciginstaller use <another carton>",
        ));
    }
    // what the rest still needs
    let mut after = cx.state.clone();
    after.cartons.remove(&name);
    let canon = |v: &str| Version::parse(v).map_or_else(|_| v.to_string(), |p| p.to_string());
    let mut needed: HashSet<(String, String)> = HashSet::new();
    for r in after.cartons.values() {
        needed.extend(r.tools.iter().map(|(t, v)| (t.clone(), canon(v))));
    }
    needed.extend(
        after
            .sticks
            .iter()
            .map(|s| (s.tool.clone(), canon(&s.version))),
    );
    needed.insert(("ciginstaller".into(), canon(env!("CARGO_PKG_VERSION"))));
    needed.insert(("ciginstaller".into(), canon(&after.installer.version)));
    if let Some(p) = &after.installer.previous {
        needed.insert(("ciginstaller".into(), canon(p)));
    }
    let gone: Vec<(String, Version)> = installed(&home)
        .into_iter()
        .filter(|(t, v)| {
            rec.tools.get(t).is_some_and(|x| canon(x) == v.to_string())
                && !needed.contains(&(t.clone(), v.to_string()))
        })
        .collect();
    if dry_run {
        let mut r = Report::done(format!(
            "dry run: would remove carton {name} and {} toolchain(s)",
            gone.len()
        ));
        for (t, v) in &gone {
            r = r.detail(format!("would remove {}", home.toolchain(t, v).display()));
        }
        return Ok(r
            .next(format!("ciginstaller remove {name}"))
            .data(json!({ "dry_run": true, "carton": name })));
    }
    let n = name.clone();
    let removed = remove_builds(
        cx,
        &format!("remove carton {name}"),
        move |s| {
            s.cartons.remove(&n);
        },
        gone.clone(),
    )?;
    let mut r = Report::done(format!(
        "carton {name} is removed, with {} toolchain(s) nothing else uses",
        gone.len()
    ));
    for p in &removed {
        r = r.detail(format!("removed {p}"));
    }
    Ok(r.next("ciginstaller list")
        .data(json!({ "carton": name, "removed": removed })))
}

// ------------------------------------------------------------------------------ --all

fn profiles(home: &Home) -> Vec<(Shell, PathBuf, PathChange)> {
    let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
    let mut out = Vec::new();
    for shell in [
        Shell::Sh,
        Shell::Bash,
        Shell::Zsh,
        Shell::Fish,
        Shell::PowerShell,
    ] {
        let (profile, _) = path_block(shell, home);
        if !seen.insert(profile.clone()) {
            continue;
        }
        if let Ok(st @ (PathChange::AlreadyPresent | PathChange::Edited)) = block_state(shell, home)
        {
            out.push((shell, profile, st));
        }
    }
    out
}

fn bin_entries(home: &Home) -> Vec<PathBuf> {
    cigi_shim::TOOLS
        .iter()
        .chain(["ciginstaller", "cigi"].iter())
        .map(|n| shim_path(home, n))
        .filter(|p| std::fs::symlink_metadata(p).is_ok())
        .collect()
}

/// The list of what is there to remove and the snapshot's entries (path, kind, sha256, bytes).
fn snapshot_entries(home: &Home) -> Vec<serde_json::Value> {
    let mut v = Vec::new();
    let root = home.root();
    let rel = |p: &std::path::Path| {
        p.strip_prefix(&root)
            .unwrap_or(p)
            .to_string_lossy()
            .replace('\\', "/")
    };
    for p in bin_entries(home) {
        let (sha, bytes) = cigi_source::digest::sha256_file(&p).unwrap_or_default();
        v.push(json!({ "path": rel(&p), "kind": "link", "sha256": sha, "bytes": bytes }));
    }
    for (t, ver) in installed(home) {
        let dir = home.toolchain(&t, &ver);
        v.push(json!({ "path": rel(&dir), "kind": "toolchain" }));
        if let Ok(r) = read_receipt(home, &t, &ver) {
            for f in r.files {
                v.push(json!({
                    "path": format!("{}/{}", rel(&dir), f.path),
                    "kind": "file", "sha256": f.sha256, "bytes": f.bytes,
                }));
            }
        }
    }
    for (id, bytes) in home.object_store().all() {
        v.push(json!({
            "path": format!("store/objects/{}/{}", &id.sha256[..2.min(id.sha256.len())], id.file_name()),
            "kind": "object", "sha256": id.sha256, "bytes": bytes,
        }));
    }
    v
}

fn all(cx: &mut Cx, a: &UninstallArgs) -> Result<Report, InstallerError> {
    let home = cx.home.clone();
    let mut lines: Vec<String> = Vec::new();

    // a run cut short after its commit point left only a journal beside the home
    let stale: Vec<_> = leftover_asides(&home)
        .into_iter()
        .filter(|x| !x.kept)
        .collect();
    if !home.installer().exists() {
        if !a.dry_run {
            for s in &stale {
                discard_aside(&home, &s.path).map_err(ie)?;
                lines.push(format!("removed {}", s.path.display()));
            }
        }
        let mut r = Report::done("nothing to uninstall: this home has no installer/ directory");
        for l in lines {
            r = r.detail(l);
        }
        return Ok(r.data(json!({ "removed": [], "changed": !stale.is_empty() })));
    }

    // what there is to remove, before anything is asked or touched
    let bins = bin_entries(&home);
    let tcs = installed(&home);
    let du = home.object_store().du();
    let profs = profiles(&home);
    let mut plan: Vec<String> = Vec::new();
    for p in &bins {
        plan.push(p.display().to_string());
    }
    for (t, v) in &tcs {
        plan.push(home.toolchain(t, v).display().to_string());
    }
    plan.push(format!(
        "{} ({} object(s), {})",
        home.store().display(),
        du.objects,
        size_text(du.bytes)
    ));
    if let Ok(es) = cigi_home::humidor::entries(&home) {
        for e in es.iter().filter(|e| e.source == cigi_home::humidor::SOURCE) {
            plan.push(home.humidor().join(&e.version).display().to_string());
        }
    }
    for (_, p, st) in &profs {
        if *st == PathChange::AlreadyPresent {
            plan.push(format!("the PATH block in {}", p.display()));
        }
    }
    plan.push(home.installer().display().to_string());

    if a.dry_run {
        let mut r = Report::done(format!(
            "dry run: would remove {} thing(s) and keep your data",
            plan.len()
        ));
        for p in &plan {
            r = r.detail(format!("would remove {p}"));
        }
        r = r.detail(not_in_build());
        return Ok(r
            .next("ciginstaller uninstall --all")
            .data(json!({ "dry_run": true, "would_remove": plan })));
    }

    let q = Question::yes_no("Remove everything ciginstaller put on this machine?", false)
        .summary(&format!(
            "{} stays, apart from what the family added",
            home.root().display()
        ))
        .detail("your run records, the tin, config and backups are kept");
    let ans = cx.ui.ask(&q);
    if !ans.is_yes(&q) {
        if ans.exit_code().is_some() {
            return Err(InstallerError::new(
                "E1871",
                "uninstall --all needs a yes and there is no terminal to ask on",
            )
            .with_fix("ciginstaller uninstall --all --yes")
            .with_exit(EXIT_USAGE));
        }
        return Ok(Report::new(Icon::Note, "nothing was removed")
            .data(json!({ "removed": [], "changed": false })));
    }

    cx.lock_and_recover()?;
    let entries = snapshot_entries(&home);
    let mut txn = cx.begin("uninstall --all")?;
    let id = txn.id().to_string();
    let mut notes: Vec<String> = Vec::new();
    let applied = (|| -> Result<PathBuf, InstallerError> {
        // the snapshot of what is about to go: it lives in installer/, so it leaves with the journal
        let snap = json!({
            "format": 1, "txn": id, "taken": now_rfc3339(),
            "home": home.root().display().to_string(), "entries": entries,
        });
        let mut text = serde_json::to_string_pretty(&snap).unwrap_or_default();
        text.push('\n');
        txn.write(
            &home.installer().join("uninstall-snapshot.json"),
            text.as_bytes(),
            Some(0o600),
        )
        .map_err(ie)?;
        remove_family(&mut txn, &home, &mut notes)?;
        move_installer_aside(&mut txn, &home).map_err(ie)
    })();
    let aside = match applied {
        Ok(p) => p,
        Err(e) => return settle(txn, Err(e)),
    };
    // installer/ is gone: that was the commit point. The commit line goes to the journal where it
    // went, and a failure to write it changes nothing about the home.
    let _ = txn.commit();
    cx.release_lock();

    let kept = a.keep_journal;
    if kept {
        let _ = cigi_home::uninstall::mark_kept(&aside);
    } else {
        let _ = discard_aside(&home, &aside);
    }
    // the home itself, when nothing of the user's is in it
    let _ = std::fs::remove_dir(home.root());

    let mut r = Report::done(format!(
        "everything ciginstaller added is removed ({} thing(s)); your data stays",
        plan.len()
    ));
    for p in &plan {
        r = r.detail(format!("removed {p}"));
    }
    for n in &notes {
        r = r.detail(n.clone());
    }
    r = r.detail(not_in_build());
    if kept {
        r = r.detail(format!("kept the journal: {}", aside.display()));
    }
    Ok(r.data(json!({
        "changed": true,
        "removed": plan,
        "notes": notes,
        "journal": if kept { json!(aside.display().to_string()) } else { json!(null) },
    })))
}

fn not_in_build() -> &'static str {
    "the guard's hooks, daemon and service, eBPF programs, WDAC policy, DNS filter and scheduled jobs are not part of this build, so there were none to remove"
}

/// Everything but `installer/`, journaled.
fn remove_family(
    txn: &mut Txn,
    home: &Home,
    notes: &mut Vec<String>,
) -> Result<(), InstallerError> {
    for p in bin_entries(home) {
        txn.delete(&p).map_err(ie)?;
    }
    if std::fs::read_dir(home.bin()).is_ok_and(|mut r| r.next().is_none()) {
        txn.delete(&home.bin()).map_err(ie)?;
    }
    remove_all_humidor_mirrors(txn, home).map_err(ie)?;
    txn.delete(&home.toolchains()).map_err(ie)?;
    txn.delete(&home.store()).map_err(ie)?;
    for (shell, profile, _) in profiles(home) {
        match remove_path_block(txn, shell, home).map_err(ie)? {
            PathChange::Edited => notes.push(format!(
                "the block in {} was edited by hand, so it is left alone (E1862): delete the lines between the cigscript markers yourself",
                profile.display()
            )),
            PathChange::Skipped => notes.push(format!(
                "{} is a link that leads nowhere usable; left alone",
                profile.display()
            )),
            _ => {}
        }
    }
    Ok(())
}
