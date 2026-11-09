// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0
//! `use <carton>`: make an installed carton the machine's. One state write and one flip; no download.

use cig_compat::{check, Combination, Origin, Pin, Severity, Version};
use cigi_home::{now_rfc3339, state::HistoryEntry};
use cigi_ui::{InstallerError, Report, EXIT_REFUSED, EXIT_USAGE};
use serde_json::json;

use super::install::finding_error;
use super::{save_and_flip, settle, yanked_in};
use crate::cx::Cx;
use crate::errors::ie;

pub struct UseArgs {
    pub carton: String,
    pub dry_run: bool,
}

/// The installed carton a name means: an exact version, or the newest installed that a pin accepts.
pub fn installed_carton(cx: &Cx, name: &str) -> Result<String, InstallerError> {
    let pin = Pin::parse(name).map_err(|e| {
        InstallerError::new("E1870", e.to_string())
            .with_fix("ciginstaller list")
            .with_exit(EXIT_USAGE)
    })?;
    let mut have: Vec<cig_compat::Version> = cx
        .state
        .cartons
        .keys()
        .filter_map(|k| cig_compat::Version::parse(k).ok())
        .filter(|v| pin.satisfied_by(v))
        .collect();
    have.sort();
    have.pop().map(|v| v.to_string()).ok_or_else(|| {
        InstallerError::new("E1816", format!("carton {name} is not installed here"))
            .with_fix(format!("ciginstaller install carton --carton {name}"))
            .with_exit(EXIT_REFUSED)
    })
}

pub fn run(cx: &mut Cx, a: &UseArgs) -> Result<Report, InstallerError> {
    cx.lock_and_recover()?;
    run_locked(cx, a)
}

pub fn run_locked(cx: &mut Cx, a: &UseArgs) -> Result<Report, InstallerError> {
    let target = installed_carton(cx, &a.carton)?;
    let old = cx.state.machine_carton.clone();
    if old.as_deref() == Some(target.as_str()) {
        return Ok(
            Report::done(format!("carton {target} is already this machine's carton"))
                .data(json!({ "carton": target, "changed": false })),
        );
    }
    let warnings = switch_checks(cx, &target, old.as_deref())?;
    if a.dry_run {
        return Ok(Report::done(format!(
            "dry run: would make carton {target} this machine's carton"
        ))
        .data(json!({ "carton": target, "dry_run": true })));
    }
    let mut state = cx.state.clone();
    let now = now_rfc3339();
    state.machine_carton = Some(target.clone());
    state.previous.machine_carton = old.clone();
    if let Some(rec) = state.cartons.get_mut(&target) {
        rec.last_used = Some(now.clone());
    }
    let mut txn = cx.begin(&format!("use carton {target}"))?;
    let id = txn.id().to_string();
    state.push_history(HistoryEntry {
        id: id.clone(),
        at: now,
        what: "use".into(),
        from: old.clone(),
        to: Some(target.clone()),
        txn: id,
        ok: true,
    });
    let home = cx.home.clone();
    let yanked = cx
        .peek_manifest()
        .map(|m| yanked_in(&m))
        .unwrap_or_default();
    let r = save_and_flip(&mut txn, &home, &state, yanked);
    settle(txn, r)?;
    cx.state = cigi_home::State::load(&cx.home).map_err(ie)?;
    let mut rep = Report::done(format!("carton {target} is now this machine's carton"))
        .data(json!({ "carton": target, "previous": old, "changed": true }));
    if let Some(o) = old {
        rep = rep.detail(format!("carton {o} stays installed"));
    }
    for w in warnings {
        rep = rep.detail(format!("warning {w}"));
    }
    Ok(rep)
}

/// What a switch must hold before it changes what runs (SPEC 12.3): the target is whole, it keeps
/// every tool the machine has now, and the marry-up check passes on it (from the cached manifest;
/// a switch never downloads, so with no cache the check is skipped). Returns the warnings.
fn switch_checks(cx: &Cx, target: &str, old: Option<&str>) -> Result<Vec<String>, InstallerError> {
    let Some(rec) = cx.state.cartons.get(target) else {
        return Ok(Vec::new());
    };
    let whole = format!("ciginstaller install carton --carton {target}");
    if rec.partial {
        return Err(InstallerError::new(
            "E1817",
            format!(
                "carton {target} is only partly installed, so it cannot be the machine's carton"
            ),
        )
        .with_hint(format!(
            "it holds {}",
            rec.tools.keys().cloned().collect::<Vec<_>>().join(", ")
        ))
        .with_fix(whole)
        .with_exit(EXIT_REFUSED));
    }
    if let Some(mrec) = old.and_then(|o| cx.state.cartons.get(o)) {
        let missing: Vec<&String> = mrec
            .tools
            .keys()
            .filter(|t| !rec.tools.contains_key(*t))
            .collect();
        if !missing.is_empty() {
            return Err(InstallerError::new(
                "E1818",
                format!(
                    "carton {target} has no {}, which this machine's carton has",
                    missing
                        .iter()
                        .map(|t| t.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )
            .with_hint("switching would leave those shims with nothing to run")
            .with_fix(whole)
            .with_exit(EXIT_REFUSED));
        }
    }
    let mut warnings = Vec::new();
    if let (Some(m), Ok(cv)) = (cx.peek_manifest(), Version::parse(target)) {
        let items: Vec<(String, Version)> = rec
            .tools
            .iter()
            .filter_map(|(t, v)| Some((t.clone(), Version::parse(v).ok()?)))
            .collect();
        for f in check(
            &m,
            &Combination {
                items,
                origin: Origin::Carton(cv),
            },
            &cx.host,
        ) {
            if f.severity == Severity::Refuse {
                return Err(finding_error(&f));
            }
            warnings.push(format!("{}: {}", f.code, f.headline));
        }
    }
    Ok(warnings)
}
