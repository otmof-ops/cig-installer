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
//! `ashtray/`, and whatever else of theirs is in `cache/` and `cigcompile/`. The products' caches
//! are not theirs and go in the same transaction (`family::all_caches`): cigar's run cache,
//! cigcompile's fetched runtimes and compile cache. Before anything, cigo's own guard is taken
//! down by `cigo guard uninstall --yes`, when it is installed. The guard's hooks, daemon, eBPF programs, WDAC policy, DNS
//! filter and scheduled jobs do not exist in this build, so there is nothing of them to remove.

use std::collections::{BTreeSet, HashSet};
use std::path::PathBuf;

use cig_compat::Version;
use cigi_home::path::block_state;
use cigi_home::state::HistoryEntry;
use cigi_home::toolchain::referenced_objects;
use cigi_home::uninstall::{discard_aside, move_installer_aside};
use cigi_home::{
    installed, now_rfc3339, path_block, read_receipt, recorded_registry, registry_path_has,
    registry_path_remove, remove_all_humidor_mirrors, remove_humidor_mirror, remove_path_block,
    remove_shims, shim_path, Home, Journal, ObjectId, PathChange, Shell, State, Txn,
};
use cigi_ui::{Icon, InstallerError, Question, Report, EXIT_REFUSED, EXIT_USAGE};
use serde_json::json;

use super::family::{
    all_caches, caches_of, delete_caches, guard_pending, guard_uninstall, only_caches,
    tidy_parents, CacheDir,
};
use super::gc::{finish_pending, record_pending};
use super::health::{pending_txns, remove_committed_txn, shim_names, size_text};
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
    caches: &[CacheDir],
) -> Result<(Vec<String>, Vec<String>), InstallerError> {
    let home = cx.home.clone();
    let before_names = shim_names(&home, &cx.state);
    let mut state = cx.state.clone();
    mutate(&mut state);
    let after_names = shim_names(&home, &state);
    let dead: Vec<String> = before_names.difference(&after_names).cloned().collect();
    let mut removed: Vec<String> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
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
        // the product's caches are not user data: they go in the same transaction, and one that
        // cannot be taken is left and said, never a reason to keep the tool
        removed.extend(delete_caches(&mut txn, &home, caches, &mut notes));
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
    tidy_parents(&home, caches);
    cx.state = State::load(&home).map_err(ie)?;
    Ok((removed, notes))
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
    // a dry run reads and takes no lock: no layout is made, nothing is recovered
    if !dry_run {
        cx.lock_and_recover()?;
    }
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
    let caches = caches_of(&home, t);
    let guard = t == "cigo" && guard_pending(&home, &cx.state);
    if dry_run {
        let mut r = Report::done(format!(
            "dry run: would remove {t} {}",
            if versions.is_empty() {
                "(its entries in state.json)".to_string()
            } else {
                versions.join(", ")
            }
        ));
        if guard {
            r = r.detail("would first run `cigo guard uninstall --yes`: cigo's guard is installed");
        }
        for (x, v) in &gone {
            r = r.detail(format!("would remove {}", home.toolchain(x, v).display()));
        }
        for c in &caches {
            r = r.detail(format!("would remove {} ({})", c.path.display(), c.what));
        }
        let interrupted = pending_txns(&home);
        for i in &interrupted {
            r = r.detail(format!(
                "an interrupted transaction is waiting: {i}; a real run settles it first"
            ));
        }
        return Ok(r.next(format!("ciginstaller uninstall {t}")).data(json!({
            "dry_run": true, "tool": t, "versions": versions, "interrupted": interrupted,
            "guard": guard,
            "caches": caches.iter().map(|c| c.path.display().to_string()).collect::<Vec<_>>(),
        })));
    }
    // cigo's guard is taken down by cigo, before cigo is: a failure stops here with nothing removed
    let guard_line = if t == "cigo" {
        guard_uninstall(&home, &cx.state)?
    } else {
        None
    };
    let tool_name = t.to_string();
    let (removed, notes) = remove_builds(
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
        &caches,
    )?;
    let mut r = Report::done(format!(
        "{t} is uninstalled: {} version(s) removed; the cartons that had it are marked partial",
        versions.len()
    ));
    if let Some(l) = &guard_line {
        r = r.detail(l.clone());
    }
    for p in &removed {
        r = r.detail(format!("removed {p}"));
    }
    for n in &notes {
        r = r.detail(n.clone());
    }
    Ok(r.next("ciginstaller list").data(json!({
        "tool": t, "versions": versions, "removed": removed, "guard": guard_line.is_some(),
        "caches": caches.iter().map(|c| c.path.display().to_string()).collect::<Vec<_>>(),
        "notes": notes,
    })))
}

fn carton(cx: &mut Cx, c: &str, dry_run: bool) -> Result<Report, InstallerError> {
    if !dry_run {
        cx.lock_and_recover()?;
    }
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
        let interrupted = pending_txns(&home);
        for i in &interrupted {
            r = r.detail(format!(
                "an interrupted transaction is waiting: {i}; a real run settles it first"
            ));
        }
        return Ok(r.next(format!("ciginstaller remove {name}")).data(json!({
            "dry_run": true, "carton": name, "interrupted": interrupted,
        })));
    }
    let n = name.clone();
    let removed = remove_builds(
        cx,
        &format!("remove carton {name}"),
        move |s| {
            s.cartons.remove(&n);
        },
        gone.clone(),
        &[],
    )?
    .0;
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
        Shell::WindowsPowerShell,
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

    if !home.installer().exists() {
        // (a journal a cut-short run left beside the home is for `gc` to discard)
        return Ok(
            Report::done("nothing to uninstall: this home has no installer/ directory")
                .data(json!({ "removed": [], "changed": false })),
        );
    }
    ours(&home)?;

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
    // the products' caches are not user data: cigar's run cache, cigcompile's fetched runtimes
    let caches = all_caches(&home);
    for c in &caches {
        plan.push(format!("{} ({})", c.path.display(), c.what));
    }
    let guard = guard_pending(&home, &cx.state);
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
    // the registry entry the install recorded as its own: exactly that entry, nothing else in the
    // value (an entry that was on the user's PATH before the install is not the installer's)
    let registry = State::load(&home)
        .ok()
        .and_then(|s| recorded_registry(&s))
        .filter(|r| !r.preexisting && registry_path_has(&r.entry).unwrap_or(false))
        .map(|r| r.entry);
    if let Some(e) = &registry {
        plan.push(format!(
            "{}: {} (the entry the install added)",
            cigi_home::path::REGISTRY_PATH,
            e.display()
        ));
    }
    plan.push(format!(
        "{} (the installer's state, your ciginstaller settings in installer/config, the manifest cache and the journals)",
        home.installer().display()
    ));
    let stays = stays(&home);

    if a.dry_run {
        let mut r = Report::done(format!(
            "dry run: would remove {} thing(s) and keep your data",
            plan.len()
        ));
        for p in &plan {
            r = r.detail(format!("would remove {p}"));
        }
        if guard {
            r = r.detail("would first run `cigo guard uninstall --yes`: cigo's guard is installed");
        }
        r = r.detail(stays_line(&home, &stays));
        r = r.detail(not_in_build());
        return Ok(r
            .next("ciginstaller uninstall --all --yes-irreversible")
            .data(json!({
                "dry_run": true, "would_remove": plan, "stays": stays, "guard": guard,
            })));
    }

    let q = Question::irreversible(&format!(
        "Remove everything ciginstaller put on {}?",
        home.root().display()
    ))
    .summary("it cannot be undone")
    .detail(&stays_line(&home, &stays))
    .detail("installer/ goes with it: the installer's state, installer/config (your ciginstaller settings), the manifest cache and the journals");
    let ans = cx.ui.ask(&q);
    if !ans.is_yes(&q) {
        if ans.exit_code().is_some() {
            return Err(InstallerError::new(
                "E1871",
                "uninstall --all needs a yes and there is no terminal to ask on",
            )
            .with_fix("ciginstaller uninstall --all --yes-irreversible")
            .with_exit(EXIT_USAGE));
        }
        return Ok(Report::new(Icon::Note, "nothing was removed")
            .data(json!({ "removed": [], "changed": false })));
    }

    // cigo takes its own guard down before anything is removed; a failure stops here, with the
    // home exactly as it was
    let guard_line = guard_uninstall(&home, &cx.state)?;
    cx.lock_and_recover()?;
    let entries = snapshot_entries(&home);
    let mut txn = cx.begin("uninstall --all")?;
    let id = txn.id().to_string();
    let mut notes: Vec<String> = Vec::new();
    if let Some(l) = &guard_line {
        notes.push(l.clone());
    }
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
        delete_caches(&mut txn, &home, &caches, &mut notes);
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

    // the registry PATH goes after the commit, like the install's add: the entry the state
    // recorded, then the broadcast (done by the removal), so a terminal opened now is clean
    if let Some(e) = &registry {
        if let Err(err) = registry_path_remove(e) {
            notes.push(format!(
                "could not take {} out of {} (E1845): {err}; remove it by hand in Settings, System, About, Advanced system settings, Environment Variables",
                e.display(),
                cigi_home::path::REGISTRY_PATH
            ));
        }
    }

    let kept = a.keep_journal;
    if kept {
        let _ = cigi_home::uninstall::mark_kept(&aside);
    } else {
        let _ = discard_aside(&home, &aside);
    }
    // Windows keeps the lock file beside `installer/`, not in it: it is the installer's, and
    // nothing holds it now
    let _ = std::fs::remove_file(home.lock_path());
    // the folders the caches stood in, when they are empty now
    tidy_parents(&home, &caches);
    // the home itself, when nothing of the user's is in it
    let _ = std::fs::remove_dir(home.root());

    let mut r = Report::done(format!(
        "everything ciginstaller added is removed ({} thing(s)); {}",
        plan.len(),
        stays_line(&home, &stays)
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
        "stays": stays,
        "notes": notes,
        "journal": if kept { json!(aside.display().to_string()) } else { json!(null) },
    })))
}

/// What is in the home that `--all` does not touch: the names of the entries of the home that
/// are not the installer's (`runs/`, `tin/`, `config`, ...), and the parts of `bin/` and
/// `humidor/` that are not the installer's.
fn stays(home: &Home) -> Vec<String> {
    let ours = [
        "bin",
        "toolchains",
        "store",
        "installer",
        "installer.lock",
        "humidor",
    ];
    let mut out: Vec<String> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(home.root()) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if ours.contains(&name.as_str()) || only_caches(home, &name) {
                continue;
            }
            let dir = e.file_type().is_ok_and(|t| t.is_dir());
            out.push(if dir { format!("{name}/") } else { name });
        }
    }
    let mine = bin_entries(home);
    if std::fs::read_dir(home.bin()).is_ok_and(|rd| rd.flatten().any(|e| !mine.contains(&e.path())))
    {
        out.push("bin/ (files in it that are not shims)".to_string());
    }
    if let Ok(es) = cigi_home::humidor::entries(home) {
        if es.iter().any(|e| e.source != cigi_home::humidor::SOURCE) {
            out.push("humidor/ (entries that are not the installer's)".to_string());
        }
    }
    out.sort();
    out
}

fn stays_line(home: &Home, stays: &[String]) -> String {
    if stays.is_empty() {
        format!("nothing else of yours is in {}", home.root().display())
    } else {
        format!(
            "stays in {}: {}; the installer's own settings and cache do not stay",
            home.root().display(),
            stays.join(", ")
        )
    }
}

/// Is this home one the installer made? Its `installer/state.json` must parse as the installer's
/// own state, and an owner marker, where there is one, must say so. A home made before the marker
/// existed has none and is accepted on its state alone. Nothing is changed or created here.
fn ours(home: &Home) -> Result<(), InstallerError> {
    let refuse = |why: String| {
        InstallerError::new(
            "E1868",
            format!(
                "{} is not a home this installer made: {why}",
                home.root().display()
            ),
        )
        .with_hint("uninstall --all removes only a home with the installer's own state in it")
        .with_fix("ciginstaller explain E1868")
        .with_exit(EXIT_REFUSED)
    };
    if std::fs::symlink_metadata(home.installer()).is_ok_and(|m| !m.is_dir()) {
        return Err(refuse("installer/ is not a directory".into()));
    }
    if let Ok(text) = std::fs::read_to_string(home.owner_marker()) {
        if !text.starts_with("ciginstaller") {
            return Err(refuse("installer/.owner says something else".into()));
        }
    }
    let text = std::fs::read_to_string(home.state_path())
        .map_err(|_| refuse("there is no installer/state.json".into()))?;
    let state: State = serde_json::from_str(&text).map_err(|e| {
        refuse(format!(
            "installer/state.json is not the installer's state ({e})"
        ))
    })?;
    if state.format == 0 {
        return Err(refuse("installer/state.json has no format".into()));
    }
    Ok(())
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
            PathChange::Emptied => notes.push(format!(
                "{} is a link, or lies outside your home, so it was not deleted: the block is gone and the file is left empty",
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
