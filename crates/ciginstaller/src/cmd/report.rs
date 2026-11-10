// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `report [<run>]` (#11): what a bug report needs, in one redacted bundle.
//!
//! - `--block` prints the version block alone (and `--block --json` is the machine form a
//!   product embeds in its own reports);
//! - with a run, the bundle also carries the installer's journal for that operation (redacted),
//!   the trial's differences and the metadata versions it saw; for a cig run, cig's own bundle;
//! - `--out FILE` writes it, `--send` files it through the crash route under the crash
//!   command's consent (`reports = never` refuses), and no identifiers travel (#19).
//!
//! The command works when the installer's state is the thing that is broken: the block then says
//! so, and the report still goes.

use std::path::{Path, PathBuf};

use cigi_ui::{Icon, InstallerError, Report, Ui};
use serde_json::{json, Value};

use super::crash as crash_cmd;
use crate::cli::ReportCmd;
use crate::crash::{self, Kind, ORIGIN};
use crate::cx::{Cx, Globals};
use crate::flows::report as flow;

pub fn run(
    ui: &Ui,
    globals: Globals,
    cwd: Option<PathBuf>,
    c: &ReportCmd,
) -> Result<Report, InstallerError> {
    let env = crash_cmd::open_env(ui, globals.home.as_ref())?;
    let cx = Cx::new(ui.duplicate(), globals, cwd);
    let mut cx = match cx {
        Ok(cx) => Some(cx),
        // a run was asked for and cannot be looked up without the state
        Err(e) if c.run.is_some() => return Err(e),
        Err(e) => {
            return finish(&env, c, flow::block_without_state(&e), None, None, None);
        }
    };
    let block = cx
        .as_mut()
        .map(flow::version_block)
        .unwrap_or_else(|| Value::Null);
    if c.block {
        return Ok(block_report(&block));
    }
    let (operation, cig, run_id) = match &cx {
        Some(cx) => gather(cx, c)?,
        None => (None, None, None),
    };
    finish(&env, c, block, operation, cig, run_id)
}

/// What a run gathered: the installer's record, cig's bundle, the run's id.
pub type Gathered = (Option<Value>, Option<Value>, Option<String>);

/// The run's two possible sources: an installer operation, else (or with `--cig`) a cig run.
fn gather(cx: &Cx, c: &ReportCmd) -> Result<Gathered, InstallerError> {
    gather_run(cx, c.run.as_deref(), c.cig)
}

/// The same, for a caller that is not `report` (`bug` gathers a run the same way).
pub fn gather_run(cx: &Cx, run: Option<&str>, cig: bool) -> Result<Gathered, InstallerError> {
    let txn = if cig {
        None
    } else {
        flow::find_operation(cx, run)?
    };
    if let Some(t) = txn {
        let op = flow::operation(&cx.home, &cx.state.history, &t);
        return Ok((Some(op), None, Some(t)));
    }
    match run {
        Some(run) => {
            let b = flow::cig_bundle(cx, run)?;
            Ok((None, Some(b), Some(run.to_string())))
        }
        None => Ok((None, None, None)),
    }
}

fn finish(
    env: &crash_cmd::Env,
    c: &ReportCmd,
    block: Value,
    operation: Option<Value>,
    cig: Option<Value>,
    run_id: Option<String>,
) -> Result<Report, InstallerError> {
    if c.block {
        return Ok(block_report(&block));
    }
    let bundle = flow::bundle(block, operation, cig, run_id.as_deref(), &env.root);
    if c.send {
        return send(env, bundle, run_id.as_deref());
    }
    if let Some(out) = &c.out {
        return write_out(out, &bundle);
    }
    let flagged = flow::flagged(&bundle["version_block"]);
    let mut r = if flagged > 0 {
        Report::new(
            Icon::Action,
            format!("report ready: {flagged} thing(s) in the version block are flagged"),
        )
    } else {
        Report::done("report ready: nothing in the version block is flagged")
    };
    for l in summary(&bundle) {
        r = r.detail(l);
    }
    let run = run_id.map(|r| format!(" {r}")).unwrap_or_default();
    Ok(r.detail("nothing is written or sent until you ask")
        .next(format!("ciginstaller report{run} --out report.json"))
        .data(json!({ "bundle": bundle })))
}

fn block_report(block: &Value) -> Report {
    let flagged = flow::flagged(block);
    let mut r = if flagged > 0 {
        Report::new(Icon::Action, format!("{flagged} thing(s) flagged"))
    } else {
        Report::done("the versions here fit together")
    };
    for l in flow::block_lines(block) {
        r = r.detail(l);
    }
    r.data(block.clone())
}

/// What the bundle holds, as lines.
fn summary(b: &Value) -> Vec<String> {
    let mut v = flow::block_lines(&b["version_block"]);
    if let Some(op) = b["operation"].as_object() {
        let t = &op["trial"];
        let trial = if t.is_object() {
            format!(
                ", trial: {} compared, {} different",
                t["compared"].as_u64().unwrap_or(0),
                t["different"].as_array().map_or(0, Vec::len)
            )
        } else {
            ", no trial recorded".to_string()
        };
        v.push(format!(
            "operation {} ({}): {} journal line(s){trial}",
            op["id"].as_str().unwrap_or("?"),
            op["status"].as_str().unwrap_or("?"),
            op["journal_lines"].as_u64().unwrap_or(0),
        ));
    }
    if b["cig_bundle"].is_object() {
        v.push("cig's own bundle for the run is included".to_string());
    }
    v
}

fn write_out(out: &Path, bundle: &Value) -> Result<Report, InstallerError> {
    let text = format!(
        "{}\n",
        serde_json::to_string_pretty(bundle).unwrap_or_default()
    );
    let bad = |e: std::io::Error| {
        flow::failed(format!("cannot write {}: {e}", out.display()))
            .with_fix("ciginstaller report --out <a file you can write>")
    };
    if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(bad)?;
    }
    write_private(out, &text).map_err(bad)?;
    let mut r = Report::done(format!("bundle written to {}", out.display()));
    for l in summary(bundle) {
        r = r.detail(l);
    }
    Ok(r.detail("read it, then attach it to an issue at https://github.com/otmof-ops/cig-installer/issues/new")
        .data(json!({
            "out": out.display().to_string(),
            "bytes": text.len(),
            "flagged": flow::flagged(&bundle["version_block"]),
        })))
}

fn write_private(p: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    o.open(p)?.write_all(text.as_bytes())
}

/// `--send`: the bundle joins the ashtray as a report of kind `report` and goes the way a crash
/// does, under the same consent: `reports = never` refuses before anything is saved, a typed
/// `--send` is the yes, and with no route the report waits and the output says so.
fn send(
    env: &crash_cmd::Env,
    bundle: Value,
    run_id: Option<&str>,
) -> Result<Report, InstallerError> {
    if crash::explicit_never(&env.cfg) {
        return Err(crash_cmd::never_error(env, None));
    }
    let what = flow::what(&bundle);
    let root = Some(env.root.as_path());
    let mut r = crash::new_report(
        ORIGIN,
        env!("CARGO_PKG_VERSION"),
        Kind::Report,
        "report",
        &what,
        Vec::new(),
        root,
    );
    r.title = Some(crash::truncate(&format!("report: {what}"), 400));
    r.active_run_id = run_id.map(|s| {
        s.chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
            .take(64)
            .collect()
    });
    r.details = Some(bundle);
    r.queue.requested = true;
    crash::write_new(&env.root, &mut r)
        .map_err(|e| flow::failed(format!("could not save the report: {e}")))?;
    crash_cmd::send(env, Some(&r.id), false)
}
