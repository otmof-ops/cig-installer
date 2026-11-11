// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `gc` (SPEC 10): remove cartons unused for `keep_days`, the toolchains nothing then needs, and
//! the store objects nothing references.
//!
//! Never removed: the machine carton, the carton `rollback` returns to, a carton whose cig is
//! named by a recent run record (`runs/*/run.json`, field `version`; a run folder holding a
//! `pinned` file counts however old it is), a stick, the installer's own current and previous
//! toolchains. A store object is freed only when no toolchain and nothing in `bin/` links to it,
//! and only after the transaction that removed its toolchain committed: a crash in between
//! leaves garbage the next `gc` frees, never damage.

use std::collections::{BTreeSet, HashSet};
use std::io::Read;
use std::path::Path;

use cig_compat::{rfc3339_to_epoch, Version};
use cigi_home::state::HistoryEntry;
use cigi_home::toolchain::{referenced_objects, remove_toolchain_now};
use cigi_home::{
    installed, now_rfc3339, read_receipt, remove_humidor_mirror, remove_shims, Home, ObjectId,
    State,
};
use cigi_ui::{InstallerError, Report, EXIT_USAGE};
use serde_json::json;

use super::{cached_yanked, save_and_flip, settle};
use crate::cx::{now_override, Cx};
use crate::errors::ie;

/// `state.json` key: the toolchains a committed change (`gc`, a rollback that undoes an update)
/// no longer uses and deletes after its commit. The index stops naming them at the flip, so no
/// shim finds one gone; the list is recorded in the change's own transaction and cleared once the
/// deletes are done. What a crash leaves, the next `gc` finishes.
pub const PENDING_KEY: &str = "pending_deletes";

/// Add `items` to the list of deletes that follow the commit (inside the caller's transaction).
pub fn record_pending(state: &mut State, items: &[(String, Version)]) {
    let mut all: Vec<(String, String)> = pending_in(state).into_iter().collect();
    for (t, v) in items {
        let e = (t.clone(), v.to_string());
        if !all.contains(&e) {
            all.push(e);
        }
    }
    all.sort();
    state.extra.insert(
        PENDING_KEY.to_string(),
        json!(all.iter().map(|(t, v)| json!([t, v])).collect::<Vec<_>>()),
    );
}

fn pending_in(state: &State) -> HashSet<(String, String)> {
    state
        .extra
        .get(PENDING_KEY)
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|e| {
            Some((
                e.get(0)?.as_str()?.to_string(),
                e.get(1)?.as_str()?.to_string(),
            ))
        })
        .collect()
}

/// After the commit: delete `items` (best effort, never one something uses again), then take the
/// ones that are gone off the list in a small transaction of its own.
pub fn finish_pending(cx: &mut Cx, items: &[(String, Version)]) -> Result<(), InstallerError> {
    cigi_home::journal::fault_after("pending-delete");
    let home = cx.home.clone();
    let state = State::load(&home).map_err(ie)?;
    let used: HashSet<(String, String)> = state
        .cartons
        .values()
        .flat_map(|r| r.tools.iter().map(|(t, v)| (t.clone(), canonical(v))))
        .chain(
            state
                .sticks
                .iter()
                .map(|s| (s.tool.clone(), canonical(&s.version))),
        )
        .collect();
    for (t, v) in items {
        if !used.contains(&(t.clone(), v.to_string())) {
            let _ = remove_toolchain_now(&home, t, v);
        }
    }
    let mut state = state;
    let before = pending_in(&state);
    let left: Vec<(String, String)> = before
        .iter()
        .filter(|(t, v)| {
            !used.contains(&(t.clone(), v.clone()))
                && Version::parse(v).is_ok_and(|ver| home.toolchain(t, &ver).exists())
        })
        .cloned()
        .collect();
    if left.len() == before.len() {
        return Ok(());
    }
    if left.is_empty() {
        state.extra.remove(PENDING_KEY);
    } else {
        let mut left = left;
        left.sort();
        state.extra.insert(
            PENDING_KEY.to_string(),
            json!(left.iter().map(|(t, v)| json!([t, v])).collect::<Vec<_>>()),
        );
    }
    let mut txn = cx.begin("the deletes after a commit")?;
    let r = state.save_in(&mut txn).map_err(ie);
    settle(txn, r)?;
    cx.state = state;
    Ok(())
}
const DAY: i64 = 24 * 3600;
/// A run record is a small JSON file; anything larger is not one.
const RUN_JSON_MAX: u64 = 1024 * 1024;

#[derive(Debug, Clone, Default)]
pub struct GcArgs {
    pub dry_run: bool,
    /// `--keep-days N`; else the `keep_days` setting.
    pub keep_days: Option<u32>,
}

/// What `gc` would do, computed without writing anything.
#[derive(Debug, Default)]
pub struct Plan {
    /// Cartons to forget, with their age in days.
    pub cartons: Vec<(String, i64)>,
    /// Toolchains to remove: those of the cartons above that nothing else needs, and old orphans.
    pub toolchains: Vec<(String, Version)>,
    /// Cartons kept although old, and why.
    pub kept: Vec<(String, String)>,
    /// Store objects that will have no reference left.
    pub objects: Vec<(ObjectId, u64)>,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.cartons.is_empty() && self.toolchains.is_empty() && self.objects.is_empty()
    }
    pub fn bytes(&self) -> u64 {
        self.objects.iter().map(|(_, n)| n).sum()
    }
}

fn now_epoch() -> i64 {
    now_override().unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64)
    })
}

/// The cig versions named by run records young enough to matter (all of them when a run is
/// pinned). Read-only: a folder, a link or a file that does not look like a run is skipped.
pub fn run_versions(home: &Home, cutoff: i64) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let Ok(rd) = std::fs::read_dir(home.root().join("runs")) else {
        return out;
    };
    for e in rd.flatten() {
        let dir = e.path();
        if !e.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let rj = dir.join("run.json");
        let Ok(md) = std::fs::symlink_metadata(&rj) else {
            continue;
        };
        if !md.is_file() || md.len() > RUN_JSON_MAX {
            continue;
        }
        let mut text = String::new();
        if std::fs::File::open(&rj)
            .and_then(|f| f.take(RUN_JSON_MAX).read_to_string(&mut text))
            .is_err()
        {
            continue;
        }
        let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        let Some(version) = val.get("version").and_then(|v| v.as_str()) else {
            continue;
        };
        let pinned = dir.join("pinned").exists();
        let when = val
            .get("started")
            .and_then(|s| s.as_str())
            .and_then(rfc3339_to_epoch)
            .or_else(|| {
                md.modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
            });
        if pinned || when.is_none_or(|w| w >= cutoff) {
            out.insert(canonical(version));
        }
    }
    out
}

/// `1.20.0` and `1.20` name the same version; anything unparsable is compared as text.
fn canonical(v: &str) -> String {
    Version::parse(v.trim()).map_or_else(|_| v.trim().to_string(), |p| p.to_string())
}

fn stamp(rec: &cigi_home::state::CartonRecord) -> Option<i64> {
    rec.last_used
        .as_deref()
        .and_then(rfc3339_to_epoch)
        .into_iter()
        .chain(rfc3339_to_epoch(&rec.installed_at))
        .max()
}

pub fn plan(cx: &Cx, keep_days: u32) -> Plan {
    let home = &cx.home;
    let state = &cx.state;
    let now = now_epoch();
    let cutoff = now - i64::from(keep_days) * DAY;
    let runs = run_versions(home, cutoff);
    let mut p = Plan::default();

    // which cartons go
    let mut gone: BTreeSet<&str> = BTreeSet::new();
    for (name, rec) in &state.cartons {
        let Some(t) = stamp(rec) else {
            continue; // cannot tell how old: keep
        };
        if t >= cutoff {
            continue;
        }
        let why = if state.machine_carton.as_deref() == Some(name.as_str()) {
            Some("the machine's carton")
        } else if state.previous.machine_carton.as_deref() == Some(name.as_str()) {
            Some("the carton rollback returns to")
        } else if rec
            .tools
            .get("cig")
            .is_some_and(|v| runs.contains(&canonical(v)))
        {
            Some("a run record names its cig")
        } else {
            None
        };
        match why {
            Some(w) => p.kept.push((name.clone(), w.to_string())),
            None => {
                gone.insert(name.as_str());
                p.cartons.push((name.clone(), (now - t) / DAY));
            }
        }
    }

    // what the rest still needs
    let mut needed: HashSet<(String, String)> = HashSet::new();
    for (name, rec) in &state.cartons {
        if !gone.contains(name.as_str()) {
            needed.extend(rec.tools.iter().map(|(t, v)| (t.clone(), canonical(v))));
        }
    }
    needed.extend(
        state
            .sticks
            .iter()
            .map(|s| (s.tool.clone(), canonical(&s.version))),
    );
    needed.insert(("ciginstaller".into(), canonical(env!("CARGO_PKG_VERSION"))));
    needed.insert(("ciginstaller".into(), canonical(&state.installer.version)));
    if let Some(prev) = &state.installer.previous {
        needed.insert(("ciginstaller".into(), canonical(prev)));
    }

    // toolchains of the cartons that go, and orphans older than the cutoff
    let of_gone: HashSet<(String, String)> = gone
        .iter()
        .filter_map(|n| state.cartons.get(*n))
        .flat_map(|r| r.tools.iter().map(|(t, v)| (t.clone(), canonical(v))))
        .collect();
    // what an earlier change committed to remove and was cut off before it did
    let pending = pending_in(state);
    for (tool, v) in installed(home) {
        let key = (tool.clone(), v.to_string());
        if needed.contains(&key) || (tool == "cig" && runs.contains(&key.1)) {
            continue;
        }
        let old_orphan = read_receipt(home, &tool, &v)
            .ok()
            .and_then(|r| rfc3339_to_epoch(&r.installed_at))
            .is_some_and(|t| t < cutoff);
        if of_gone.contains(&key) || old_orphan || pending.contains(&key) {
            p.toolchains.push((tool, v));
        }
    }

    // objects nothing will reference: those already loose, and those only the removed use
    let removed: HashSet<(String, Version)> = p.toolchains.iter().cloned().collect();
    let refs = referenced_objects(home);
    let store = home.object_store();
    for (id, bytes) in store.all() {
        let freed = match refs.get(&id) {
            None => true,
            Some(who) => who.iter().all(|w| removed.contains(w)),
        };
        if freed {
            p.objects.push((id, bytes));
        }
    }
    p
}

fn names_of(home: &Home, state: &State) -> BTreeSet<String> {
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
        for s in cigi_shim::shims_of(t) {
            names.insert(s.to_string());
        }
        if let Some(r) = Version::parse(v)
            .ok()
            .and_then(|v| read_receipt(home, t, &v).ok())
        {
            names.extend(
                r.shims
                    .iter()
                    .map(|s| s.name.clone())
                    .filter(|n| cigi_shim::tool_of(n) == Some(t.as_str())),
            );
        }
    }
    names
}

fn size_text(n: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut x = n as f64;
    let mut u = 0;
    while x >= 1024.0 && u < UNITS.len() - 1 {
        x /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{x:.1} {}", UNITS[u])
    }
}

pub fn run(cx: &mut Cx, a: &GcArgs) -> Result<Report, InstallerError> {
    cx.lock_and_recover()?;
    // the journals a cut-short `uninstall --all` left beside the home (E1865), whether or not
    // `installer/` has been made again since; only the exact sibling name is ever removed
    let mut leftovers: Vec<String> = Vec::new();
    for s in cigi_home::uninstall::leftover_asides(&cx.home)
        .into_iter()
        .filter(|s| !s.kept)
    {
        if !a.dry_run {
            cigi_home::uninstall::discard_aside(&cx.home, &s.path).map_err(ie)?;
        }
        leftovers.push(s.path.display().to_string());
    }
    let mut r = run_locked(cx, a)?;
    let verb = if a.dry_run { "would remove" } else { "removed" };
    for l in &leftovers {
        r = r.detail(format!(
            "{verb} {l} (the journal of an interrupted uninstall)"
        ));
    }
    if let Some(o) = r.data.as_object_mut() {
        o.insert("uninstall_journals".into(), json!(leftovers));
    }
    Ok(r)
}

pub fn run_locked(cx: &mut Cx, a: &GcArgs) -> Result<Report, InstallerError> {
    let keep_days = match a.keep_days {
        Some(n) => n,
        None => cx.cfg.get("keep_days").parse::<u32>().map_err(|_| {
            InstallerError::new("E1870", "`keep_days` is a whole number of days")
                .with_fix("ciginstaller config set keep_days 90")
                .with_exit(EXIT_USAGE)
        })?,
    };
    let p = plan(cx, keep_days);
    let describe = |mut r: Report, verb: &str| -> Report {
        for (c, age) in &p.cartons {
            r = r.detail(format!("{verb} carton {c} (unused for {age} days)"));
        }
        for (t, v) in &p.toolchains {
            r = r.detail(format!("{verb} {t} {v}"));
        }
        for (c, why) in &p.kept {
            r = r.detail(format!("keeps carton {c}: {why}"));
        }
        r
    };
    let data = |dry: bool| {
        json!({
            "dry_run": dry,
            "keep_days": keep_days,
            "cartons": p.cartons.iter().map(|(c, age)| json!({ "carton": c, "unused_days": age })).collect::<Vec<_>>(),
            "toolchains": p.toolchains.iter().map(|(t, v)| json!({ "tool": t, "version": v.to_string() })).collect::<Vec<_>>(),
            "kept": p.kept.iter().map(|(c, why)| json!({ "carton": c, "why": why })).collect::<Vec<_>>(),
            "objects": p.objects.len(),
            "bytes": p.bytes(),
        })
    };
    if p.is_empty() {
        return Ok(describe(
            Report::done(format!(
                "nothing to remove (cartons unused for more than {keep_days} days, and store objects nothing uses)"
            )),
            "would remove",
        )
        .data(data(a.dry_run)));
    }
    if a.dry_run {
        let head = format!(
            "dry run: would remove {} carton(s), {} toolchain(s) and free {} store object(s), {}",
            p.cartons.len(),
            p.toolchains.len(),
            p.objects.len(),
            size_text(p.bytes())
        );
        return Ok(describe(Report::done(head), "would remove")
            .next("ciginstaller gc")
            .data(data(true)));
    }

    let home = cx.home.clone();
    let before_names = names_of(&home, &cx.state);
    let mut state = cx.state.clone();
    for (c, _) in &p.cartons {
        state.cartons.remove(c);
    }
    let after_names = names_of(&home, &state);
    let mut txn = cx.begin("gc")?;
    let txn_id = txn.id().to_string();
    let applied = (|| -> Result<(), InstallerError> {
        for (t, v) in &p.toolchains {
            if t == "cig" {
                remove_humidor_mirror(&mut txn, &home, v).map_err(ie)?;
            }
        }
        let dead: Vec<&str> = before_names
            .difference(&after_names)
            .map(String::as_str)
            .collect();
        remove_shims(&mut txn, &home, &dead).map_err(ie)?;
        // the index flip below stops naming these; the directories go after the commit, so no
        // shim ever finds an index that points at a toolchain already deleted
        record_pending(&mut state, &p.toolchains);
        let now = now_rfc3339();
        state.gc.last_run = Some(now.clone());
        state.push_history(HistoryEntry {
            id: txn_id.clone(),
            at: now,
            what: "gc".into(),
            from: None,
            to: None,
            txn: txn_id.clone(),
            ok: true,
        });
        save_and_flip(&mut txn, &home, &state, cached_yanked(cx))
    })();
    settle(txn, applied)?;

    // After the commit: free what nothing references now (idempotent; a crash here is garbage
    // for the next run), then drop this transaction's saved copies, which still hold the bytes of
    // the toolchains it removed.
    finish_pending(cx, &p.toolchains)?;
    let store = home.object_store();
    let keep: HashSet<ObjectId> = referenced_objects(&home).into_keys().collect();
    let (mut freed, mut bytes) = (0usize, 0u64);
    for (id, n) in store.unreferenced(&keep) {
        if store.remove(&id).is_ok() {
            freed += 1;
            bytes += n;
        }
    }
    store.sweep_tmp();
    remove_committed_txn(&home, &txn_id);
    cx.state = State::load(&home).map_err(ie)?;

    let mut r = describe(
        Report::done(format!(
            "removed {} carton(s) and {} toolchain(s); freed {} store object(s), {}",
            p.cartons.len(),
            p.toolchains.len(),
            freed,
            size_text(bytes)
        )),
        "removed",
    );
    r = r.data({
        let mut d = data(false);
        d["objects"] = json!(freed);
        d["bytes"] = json!(bytes);
        d
    });
    Ok(r)
}

fn remove_committed_txn(home: &Home, id: &str) {
    let dir: std::path::PathBuf = home.txn_root().join(id);
    if is_plain_id(id) && dir.starts_with(home.txn_root()) {
        let _ = std::fs::remove_dir_all(&dir);
    }
}

fn is_plain_id(id: &str) -> bool {
    !id.is_empty()
        && !id.contains(['/', '\\'])
        && id != "."
        && id != ".."
        && Path::new(id).is_relative()
}
