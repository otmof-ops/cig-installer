// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `rollback [tool]` (SPEC 12.3): back to the previous carton, or one tool's build in it. It is a
//! switch: lock, recover, verify the toolchains it goes to (repairing from the store, else
//! `E1831`), new state, shims, the index flip.
//!
//! When the previous carton is the one the last `update` left and that update installed the
//! carton we are leaving, the rollback undoes the update: the carton it added is dropped from
//! `state.json` (and its toolchains, when nothing else uses them), `state.previous` is what it was
//! before the update, so the state differs only in `history` and the manifest record.

use cig_compat::{check, Combination, Origin, Severity, Version};
use cigi_home::state::{HistoryEntry, Previous, Stick, ROLLBACK_STICK_PREFIX};
use cigi_home::{now_rfc3339, remove_humidor_mirror, State, Txn};
use cigi_ui::{InstallerError, Report, EXIT_REFUSED};
use serde_json::json;

use super::install::finding_error;
use super::switch::{resync_shims, verify_or_repair, UpdateUndo, UNDO_KEY};
use super::{cached_yanked, save_and_flip, settle};
use crate::cx::Cx;
use crate::errors::ie;

#[derive(Debug, Clone, Default)]
pub struct RollbackArgs {
    pub tool: Option<String>,
    pub dry_run: bool,
}

fn nothing(why: &str) -> InstallerError {
    InstallerError::new("E1852", why)
        .with_fix("ciginstaller list")
        .with_hint("`ciginstaller history` shows what changed")
        .with_exit(EXIT_REFUSED)
}

pub fn run(cx: &mut Cx, a: &RollbackArgs) -> Result<Report, InstallerError> {
    cx.lock_and_recover()?;
    let Some(cur) = cx.state.machine_carton.clone() else {
        return Err(nothing(
            "nothing is installed, so there is nothing to roll back",
        ));
    };
    let Some(prev) = cx.state.previous.machine_carton.clone() else {
        return Err(nothing(
            "there is no previous carton: no update has happened since install",
        ));
    };
    let Some(prec) = cx.state.cartons.get(&prev).cloned() else {
        return Err(nothing(&format!(
            "the previous carton {prev} is no longer installed"
        )));
    };
    if let Some(t) = &a.tool {
        return tool_back(cx, a, &cur, &prev, t, &prec.tools);
    }
    let undo = UpdateUndo::read(&cx.state)
        .filter(|u| u.applies(&cx.state) && u.added == cur && u.from == prev);
    if a.dry_run {
        let mut r = Report::done(format!("dry run: would roll back carton {cur} to {prev}"));
        if undo.is_some() {
            r = r.detail(format!(
                "carton {cur} was installed by the last update: it would be removed again"
            ));
        }
        return Ok(r.data(json!({ "from": cur, "to": prev, "dry_run": true })));
    }
    let before = cx.state.clone();
    let mut txn = cx.begin(&format!("rollback to carton {prev}"))?;
    let id = txn.id().to_string();
    let r = apply_carton(cx, &mut txn, &before, &cur, &prev, undo.as_ref());
    let doomed = settle(txn, r)?;
    // the toolchains the undo leaves unused go after the commit: no shim ever finds an index that
    // points at a deleted toolchain
    super::gc::finish_pending(cx, &doomed)?;
    cx.state = State::load(&cx.home).map_err(ie)?;
    let mut rep = Report::done(format!("rolled back carton {cur} to {prev}"));
    if undo.is_some() {
        rep = rep.detail(format!(
            "carton {cur} was installed by the update, and is removed again"
        ));
    } else {
        rep = rep.detail(format!(
            "carton {cur} stays installed; `ciginstaller use {cur}` goes forward again"
        ));
    }
    Ok(rep.data(json!({ "from": cur, "to": prev, "txn": id, "undone_update": undo.is_some() })))
}

fn apply_carton(
    cx: &Cx,
    txn: &mut Txn,
    before: &State,
    cur: &str,
    prev: &str,
    undo: Option<&UpdateUndo>,
) -> Result<Vec<(String, Version)>, InstallerError> {
    let home = cx.home.clone();
    let mut doomed: Vec<(String, Version)> = Vec::new();
    let prec = before.cartons.get(prev).ok_or_else(|| {
        nothing(&format!(
            "the previous carton {prev} is no longer installed"
        ))
    })?;
    // the old toolchains are verified before the flip; a damaged one is repaired from the store
    for (t, v) in &prec.tools {
        if let Ok(v) = Version::parse(v) {
            verify_or_repair(txn, &home, t, &v)?;
        }
    }
    let mut state = before.clone();
    state.machine_carton = Some(prev.to_string());
    state.sticks.retain(|s| !s.is_rollback());
    match undo {
        Some(u) => {
            let gone = state.cartons.remove(cur);
            state.previous = u.previous.clone();
            state.extra.remove(UNDO_KEY);
            if let Some(g) = gone {
                for (t, v) in &g.tools {
                    let used = state.cartons.values().any(|r| r.tools.get(t) == Some(v))
                        || state.sticks.iter().any(|s| &s.tool == t && &s.version == v);
                    if used {
                        continue;
                    }
                    if let Ok(v) = Version::parse(v) {
                        if t == "cig" {
                            remove_humidor_mirror(txn, &home, &v).map_err(ie)?;
                        }
                        doomed.push((t.clone(), v));
                    }
                }
            }
        }
        None => {
            state.previous = Previous {
                machine_carton: Some(cur.to_string()),
                kept_until: None,
            };
        }
    }
    let now = now_rfc3339();
    state.push_history(HistoryEntry {
        id: txn.id().to_string(),
        at: now,
        what: "rollback".into(),
        from: Some(cur.to_string()),
        to: Some(prev.to_string()),
        txn: txn.id().to_string(),
        ok: true,
    });
    resync_shims(txn, &home, before, &state)?;
    // the toolchains the undo leaves unused are deleted after the commit, recorded here so a
    // crash between the two is finished by the next `gc`
    super::gc::record_pending(&mut state, &doomed);
    save_and_flip(txn, &home, &state, cached_yanked(cx))?;
    Ok(doomed)
}

/// `rollback <tool>`: only that tool's build in the previous carton. The machine becomes a mix,
/// which is recorded as a stick and checked, with a warning.
fn tool_back(
    cx: &mut Cx,
    a: &RollbackArgs,
    cur: &str,
    prev: &str,
    tool: &str,
    prev_tools: &std::collections::BTreeMap<String, String>,
) -> Result<Report, InstallerError> {
    let Some(pv) = prev_tools.get(tool) else {
        return Err(InstallerError::new(
            "E1852",
            format!("the previous carton {prev} has no {tool} to roll back to"),
        )
        .with_fix("ciginstaller list")
        .with_exit(EXIT_REFUSED));
    };
    let build = Version::parse(pv).map_err(|e| ie_msg(format!("{tool} {pv}: {e}")))?;
    let now_build = cx
        .state
        .cartons
        .get(cur)
        .and_then(|r| r.tools.get(tool))
        .cloned();
    if now_build.as_deref() == Some(pv.as_str()) {
        return Ok(
            Report::done(format!("{tool} {pv} is already what the machine runs"))
                .data(json!({ "tool": tool, "version": pv, "changed": false })),
        );
    }
    // the marry-up check of the mix, from the cached manifest
    let mut warnings = vec![format!(
        "the machine is now a mix: {tool} {pv} from carton {prev}, everything else from carton {cur}"
    )];
    if let (Some(m), Ok(cv)) = (cx.peek_manifest(), Version::parse(cur)) {
        let mut items: Vec<(String, Version)> = cx
            .state
            .cartons
            .get(cur)
            .map(|r| {
                r.tools
                    .iter()
                    .filter_map(|(t, v)| Some((t.clone(), Version::parse(v).ok()?)))
                    .collect()
            })
            .unwrap_or_default();
        items.retain(|(t, _)| t != tool);
        items.push((tool.to_string(), build.clone()));
        for f in check(
            &m,
            &Combination {
                items,
                origin: Origin::Stick { carton: cv },
            },
            &cx.host,
        ) {
            if f.severity == Severity::Refuse {
                return Err(finding_error(&f));
            }
            warnings.push(format!("{}: {}", f.code, f.headline));
        }
    }
    if a.dry_run {
        let mut r = Report::done(format!(
            "dry run: would roll {tool} back to {pv} (from carton {prev})"
        ));
        for w in warnings {
            r = r.detail(format!("warning {w}"));
        }
        return Ok(r.data(json!({ "tool": tool, "version": pv, "dry_run": true })));
    }
    let before = cx.state.clone();
    let mut txn = cx.begin(&format!("rollback {tool} to {pv}"))?;
    let id = txn.id().to_string();
    let r = (|| {
        let home = cx.home.clone();
        verify_or_repair(&mut txn, &home, tool, &build)?;
        let mut state = before.clone();
        let now = now_rfc3339();
        state.sticks.retain(|s| s.tool != tool);
        state.sticks.push(Stick {
            tool: tool.to_string(),
            version: pv.clone(),
            from_carton: prev.to_string(),
            installed_at: now.clone(),
            reason: format!("{ROLLBACK_STICK_PREFIX}{tool}"),
        });
        state.push_history(HistoryEntry {
            id: txn.id().to_string(),
            at: now,
            what: format!("rollback {tool}"),
            from: now_build.clone(),
            to: Some(pv.clone()),
            txn: txn.id().to_string(),
            ok: true,
        });
        resync_shims(&mut txn, &home, &before, &state)?;
        save_and_flip(&mut txn, &home, &state, cached_yanked(cx))
    })();
    settle(txn, r)?;
    cx.state = State::load(&cx.home).map_err(ie)?;
    let mut rep = Report::done(format!("rolled {tool} back to {pv}, from carton {prev}"));
    for w in warnings {
        rep = rep.detail(format!("warning {w}"));
    }
    Ok(rep
        .next("ciginstaller which --why")
        .data(json!({ "tool": tool, "version": pv, "from_carton": prev, "txn": id })))
}

fn ie_msg(m: String) -> InstallerError {
    crate::errors::bug(m)
}
