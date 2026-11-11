// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `which <tool> [--why] [--path]`, and `which [--why]` with no tool: every tool of the family,
//! what runs here and why. (#30 also wants a row per project; that comes with the project walk.)

use std::path::Path;

use cigi_home::journal::{list_txns, TxnStatus};
use cigi_home::{now_rfc3339, State};
use cigi_ui::{Align, Cell, InstallerError, Report, Table, EXIT_REFUSED, EXIT_USAGE};
use serde_json::json;

use super::tabular::details;
use super::Outcome;
use crate::cx::Cx;
use crate::resolve::{resolve_shim, Mode, Resolution};

pub fn run(
    cx: &mut Cx,
    tool: Option<&str>,
    why: bool,
    path_only: bool,
    script: Option<&Path>,
) -> Result<Outcome, InstallerError> {
    match tool {
        Some(t) => one(cx, t, why, path_only, script),
        None if script.is_some() => Err(InstallerError::new(
            "E1870",
            "--script needs a tool: `ciginstaller which cig --script <file>`",
        )
        .with_fix("ciginstaller which cig --script <file>")
        .with_exit(EXIT_USAGE)),
        None if path_only => Err(InstallerError::new(
            "E1870",
            "--path needs a tool: `ciginstaller which <tool> --path`",
        )
        .with_fix("ciginstaller which <tool> --path")
        .with_exit(EXIT_USAGE)),
        None => all(cx, why),
    }
}

/// Every tool of the family: what runs here and why. A tool that cannot be resolved gets a row
/// saying so, with its code; the command still answers, and its next step is the first fix.
fn all(cx: &mut Cx, why: bool) -> Result<Outcome, InstallerError> {
    let mut table = Table::new(&[
        ("tool", Align::Left),
        ("runs", Align::Left),
        ("from", Align::Left),
        ("why", Align::Left),
    ]);
    let mut rows = Vec::new();
    let mut steps: Vec<String> = Vec::new();
    let mut resolved = 0;
    let mut first_fix: Option<String> = None;
    for (shim, _tool) in cigi_shim::SHIMS {
        let tool = shim;
        match resolve_shim(cx, shim, Mode::Which, None) {
            Ok(res) => {
                // No `touch_last_used` here: this is a survey of every tool, it must leave
                // `state.json` alone (the one-tool form keeps its once-a-day touch).
                resolved += 1;
                let from = res
                    .carton
                    .as_ref()
                    .map_or("a stick".to_string(), |c| format!("carton {c}"));
                table = table.row(vec![
                    Cell::new(tool),
                    Cell::new(res.build.to_string()),
                    Cell::new(from),
                    Cell::new(res.why_text()),
                ]);
                if why {
                    steps.push(format!("{tool}:"));
                    steps.extend(res.steps.iter().map(|s| format!("  {}", s.trim_end())));
                }
                rows.push(json!({
                    "tool": tool,
                    "version": res.build.to_string(),
                    "carton": res.carton.as_ref().map(|c| c.to_string()),
                    "path": res.path.display().to_string(),
                    "why": res.why_text(),
                    "steps": res.steps,
                }));
            }
            Err(e) => {
                table = table.row(vec![
                    Cell::new(tool),
                    Cell::changed("nothing"),
                    Cell::new("-"),
                    // short on purpose: the last column is cut to the terminal's width, and
                    // the whole message is in `--json`
                    Cell::new(if e.code == "E1840" {
                        format!("{}: not installed", e.code)
                    } else {
                        format!("{}: {}", e.code, e.message)
                    }),
                ]);
                if first_fix.is_none() {
                    first_fix = e.fix_text();
                }
                rows.push(json!({
                    "tool": tool,
                    "version": null,
                    "carton": null,
                    "path": null,
                    "why": null,
                    "code": e.code,
                    "error": e.message,
                }));
            }
        }
    }
    if resolved == 0 && cx.state.cartons.is_empty() && cx.state.sticks.is_empty() {
        return Ok(Report::done("nothing is installed, so nothing runs here")
            .next("ciginstaller install carton")
            .data(json!({ "tools": rows }))
            .into());
    }
    let mut r = Report::done(format!(
        "{resolved} of {} tools run from here",
        cigi_shim::SHIMS.len()
    ));
    for l in details(&cx.ui, &table) {
        r = r.detail(l);
    }
    for l in steps {
        r = r.detail(l);
    }
    if let Some(f) = first_fix {
        r = r.next(f);
    }
    Ok(r.data(json!({ "tools": rows })).into())
}

fn one(
    cx: &mut Cx,
    tool: &str,
    why: bool,
    path_only: bool,
    script: Option<&Path>,
) -> Result<Outcome, InstallerError> {
    // Exit rule: only the codes that mean "refused" (a pin or CIGI_CARTON the machine cannot
    // meet: E1816, E1842, E1801) exit 2. Everything else keeps its own code: E1840 (no tool) is
    // 127, E1831 and the rest follow the registry, E1890 (a bug) is 70.
    let res = resolve_shim(cx, tool, Mode::Which, script).map_err(|mut e| {
        if matches!(e.code, "E1816" | "E1842" | "E1801" | "E1809" | "E1819") {
            e.exit = EXIT_REFUSED;
        }
        // `--why` still says how far the look got, so the error is not the whole story
        if why && !cx.last_steps.is_empty() {
            let mut r = Report::new(cigi_ui::Icon::Note, format!("how {tool} was looked for"));
            for s in &cx.last_steps {
                r = r.detail(s.clone());
            }
            cx.ui.say(&r);
        }
        e
    })?;
    touch_last_used(cx, &res);
    if path_only {
        return Ok(Outcome::Page(res.path.display().to_string()));
    }
    let mut r = Report::done(res.text(tool)).detail(res.path.display().to_string());
    if why {
        for s in &res.steps {
            r = r.detail(s.clone());
        }
    }
    let mut data = json!({
        "tool": tool,
        "version": res.build.to_string(),
        "carton": res.carton.as_ref().map(|c| c.to_string()),
        "path": res.path.display().to_string(),
        "steps": res.steps,
    });
    if let (Some(s), Some(o)) = (&res.script, data.as_object_mut()) {
        o.insert(
            "script".into(),
            json!({ "file": s.file.display().to_string(), "header": s.header }),
        );
    }
    Ok(r.data(data).into())
}

/// `last_used` moves at most once a day, only when the lock is free, and never while a journal
/// is unfinished: `recover` owns `state.json` then, and a write here would be undone or would
/// disagree with the journal's recorded hash. Losing the race is fine.
pub fn touch_last_used(cx: &Cx, res: &Resolution) {
    let Some(c) = &res.carton else { return };
    let key = c.to_string();
    let today = now_rfc3339();
    let day = today.get(..10).unwrap_or("");
    let stale = cx
        .state
        .cartons
        .get(&key)
        .is_some_and(|r| r.last_used.as_deref().and_then(|l| l.get(..10)) != Some(day));
    if !stale {
        return;
    }
    let Ok(_lock) = cx.home.lock_with(std::time::Duration::ZERO, &mut |_| {}) else {
        return;
    };
    if list_txns(&cx.home)
        .iter()
        .any(|t| matches!(t.status, TxnStatus::Open | TxnStatus::Unreadable))
    {
        return;
    }
    let Ok(mut state) = State::load(&cx.home) else {
        return;
    };
    let Some(rec) = state.cartons.get_mut(&key) else {
        return;
    };
    rec.last_used = Some(today);
    let _ = write_atomic(&cx.home.state_path(), state.to_json().as_bytes());
}

/// A `create_new` temp with an unguessable name (it never follows a planted link), fsync, rename,
/// fsync of the directory; the file keeps the mode `state.json` already had.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::hash::{BuildHasher, Hasher};
    use std::io::Write;
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u32(std::process::id());
    let tmp = path.with_file_name(format!(".state.json.tmp-{:016x}", h.finish()));
    let res = (|| {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        if let Ok(m) = std::fs::metadata(path) {
            let _ = f.set_permissions(m.permissions());
        }
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    match res {
        Ok(()) => {
            if let Some(dir) = path.parent() {
                let _ = cigi_home::platform::sync_dir(dir);
            }
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}
