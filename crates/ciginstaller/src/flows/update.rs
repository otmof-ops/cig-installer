// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `update` (SPEC 12.2): check, choose, plan, fetch only what changed, materialize it beside the
//! old, trial it on the project, then switch (state, `state.previous`, the index flip), as one
//! transaction. Everything before the switch leaves the machine as it was.

use std::path::{Path, PathBuf};

use cig_compat::{
    check, Carton, ChangeKind, Combination, Manifest, Origin, Pin, Severity, ToolChange, Version,
};
use cigi_home::state::{CartonRecord, HistoryEntry, ManifestInfo, Previous};
use cigi_home::{now_rfc3339, prune_txns, read_receipt, Journal, State, Txn};
use cigi_shim::{trusted_entry, trusted_for};
use cigi_source::TrustLevel;
use cigi_ui::{Choice, Icon, InstallerError, Question, Report, EXIT_REFUSED, EXIT_USAGE};
use serde_json::json;

use super::install::{fetch_builds, finding_error, materialize_fetched, Fetched};
use super::switch::{resync_shims, UpdateUndo, UNDO_KEY};
use super::trial::{self, Resolved, TrialOutcome};
use super::{present, save_and_flip, self_update, settle, tool_exe, yanked_in};
use crate::cmd::install::no_such_channel;
use crate::cx::{Cx, ManifestMode};
use crate::errors::{bug, ie};
use crate::project::{self, ProjectCtx};

/// Exit code of `update --check --exit-code` when a newer carton exists (SPEC 11.2).
pub const EXIT_NEWER: i32 = 10;

#[derive(Debug, Clone, Default)]
pub struct UpdateArgs {
    pub check: bool,
    pub exit_code: bool,
    pub refresh: bool,
    pub to: Option<String>,
    pub allow_downgrade: bool,
    pub no_trial: bool,
    pub trial_only: bool,
    pub yes_differences: bool,
    pub channel: Option<String>,
    pub project: Option<PathBuf>,
    pub dry_run: bool,
}

/// A report and the exit code to end with when it is not 0 (`--check --exit-code`: 10).
pub struct Updated {
    pub report: Report,
    pub exit: i32,
}

fn done(report: Report) -> Result<Updated, InstallerError> {
    Ok(Updated { report, exit: 0 })
}

fn refused(code: &'static str, msg: impl Into<String>, fix: &str) -> InstallerError {
    InstallerError::new(code, msg)
        .with_fix(fix)
        .with_exit(EXIT_REFUSED)
}

/// SPEC 12.2 step 1: `self update` first, when the manifest holds a newer installer, and carry on
/// with the new binary by re-exec, with the same arguments. A dev build (not `bin/ciginstaller`)
/// never replaces itself, so it updates the cartons only.
///
/// A self-update that fails with E1853 (the new build did not pass its self-check, and the old one
/// is back) or E1815 (no build for this platform) is a note: the cartons still update with the
/// running binary. Any other error stops the update.
fn self_update_first(cx: &mut Cx) -> Result<(), InstallerError> {
    match self_update_step(cx) {
        Err(e) if e.code == "E1853" || e.code == "E1815" => {
            cx.note(&format!(
                "{}: {}; updating the cartons with this ciginstaller",
                e.code, e.message
            ));
            Ok(())
        }
        r => r,
    }
}

fn self_update_step(cx: &mut Cx) -> Result<(), InstallerError> {
    // the new build a self-update just started skips the step, once: the flag rides on that one
    // command line (nothing in the environment or a profile can carry it to another run), and it
    // counts only when it names the running version
    let marked = cx.globals.self_updated.as_deref() == Some(env!("CARGO_PKG_VERSION"));
    if marked || !self_update::is_managed(&cx.home) {
        return Ok(());
    }
    let m = cx.manifest(ManifestMode::Fresh)?.clone();
    if self_update::newer_installer(&m, &cx.target).is_none() {
        return Ok(());
    }
    if !cfg!(unix) {
        cx.note("a newer ciginstaller is offered, and self update is not built for this platform yet; updating cartons only");
        return Ok(());
    }
    let rep = self_update::run_locked(cx, &self_update::SelfArgs { check: false })?;
    cx.ui.say(&rep);
    reexec(cx)
}

/// Run `bin/ciginstaller` in place of this process with the same arguments. The installer lock is
/// let go first; the file it is held on closes on exec anyway (std opens with O_CLOEXEC).
#[cfg(unix)]
fn reexec(cx: &mut Cx) -> Result<(), InstallerError> {
    use std::os::unix::process::CommandExt;
    cx.release_lock();
    let exe = cigi_home::shim_path(&cx.home, "ciginstaller");
    // the flag names the version just installed: only that binary skips the step
    let installed = State::load(&cx.home)
        .map(|s| s.installer.version)
        .unwrap_or_default();
    let err = std::process::Command::new(&exe)
        .arg0("ciginstaller")
        .arg(format!("--self-updated={installed}"))
        .args(std::env::args_os().skip(1))
        .exec();
    Err(InstallerError::new(
        "E1853",
        format!("the updated installer could not be started: {err}"),
    )
    .with_fix("ciginstaller update"))
}
#[cfg(not(unix))]
fn reexec(_cx: &mut Cx) -> Result<(), InstallerError> {
    Ok(())
}

pub fn run(cx: &mut Cx, a: &UpdateArgs) -> Result<Updated, InstallerError> {
    if let Some(ch) = &a.channel {
        cx.cfg
            .set_for_run("channel", ch)
            .map_err(|_| no_such_channel(ch))?;
    }
    if cx.cfg.get("update_check") == "off" {
        return done(
            Report::done("update checks are off")
                .detail("update_check = off sends no request at all")
                .next("ciginstaller config set update_check on"),
        );
    }
    if !a.check {
        cx.lock_and_recover()?;
        // only a real update replaces the installer: a dry run or a trial writes nothing
        if !a.dry_run && !a.trial_only {
            self_update_first(cx)?;
        }
    }
    let Some(machine) = cx.state.machine_carton.clone() else {
        return Err(refused(
            "E1840",
            "nothing is installed: there is no carton to update",
            "ciginstaller install carton",
        ));
    };
    let cur = Version::parse(&machine).map_err(|e| {
        bug(format!(
            "the machine carton `{machine}` does not parse: {e}"
        ))
    })?;

    // check: a real update fetches now; `--check` uses the daily cache unless `--refresh`
    let mode = if a.check && !a.refresh {
        ManifestMode::Daily
    } else {
        ManifestMode::Fresh
    };
    let m: Manifest = cx.manifest(mode)?.clone();

    // choose
    let target = match choose(&m, &cur, a)? {
        Pick::Target(c) => c,
        Pick::Nothing(r) => return done(r),
    };
    let tv = target.version.clone();
    let changes = changes_of(&m, &cur, &target, &cx.state);
    let want: Vec<(String, Version)> = target
        .products
        .iter()
        .filter(|(t, _)| t.as_str() != "ciginstaller")
        .map(|(t, v)| (t.clone(), v.clone()))
        .collect();

    // plan: refusals and the marry-up check of the result, before a byte is fetched
    // --check, --dry-run and --trial-only write nothing, so they show every advisory and remember none
    let read_only = a.check || a.dry_run || a.trial_only;
    let (mut warnings, shown) = plan_checks(cx, &m, &cur, &tv, &changes, &want, read_only)?;
    let pin_note = project_note(cx, a, &tv);
    if let Some(n) = &pin_note {
        warnings.push(n.clone());
    }
    let plan_lines: Vec<String> = changes.iter().map(change_line).collect();
    let newer = tv > cur;
    let fresh = whats_new(&target, &changes);
    let links = whats_new_links(
        &m,
        &changes,
        &format!("{}-{}", cx.target.os, cx.target.arch),
    );

    if a.check {
        let mut r = if newer {
            Report::done(format!(
                "carton {tv} is out; this machine is on carton {cur}"
            ))
            .next("ciginstaller update")
        } else {
            Report::done(format!(
                "carton {tv} is available, and is not newer than {cur}"
            ))
        };
        if newer {
            r = r.detail(fresh.clone());
            for l in &links {
                r = r.detail(l.clone());
            }
        }
        for l in &plan_lines {
            r = r.detail(l.clone());
        }
        for w in &warnings {
            r = r.detail(w.clone());
        }
        let r = r.data(json!({
            "current": cur.to_string(), "target": tv.to_string(), "newer": newer,
            "changes": plan_lines, "check": true, "whats_new": fresh, "links": links,
        }));
        return Ok(Updated {
            report: r,
            exit: if a.exit_code && newer { EXIT_NEWER } else { 0 },
        });
    }

    let todo: Vec<(String, Version)> = want
        .iter()
        .filter(|(t, v)| !present(&cx.home, t, v))
        .cloned()
        .collect();
    if a.dry_run {
        let mut r = Report::done(format!("dry run: would update carton {cur} to {tv}"));
        for l in &plan_lines {
            r = r.detail(l.clone());
        }
        for (t, v) in &want {
            let verb = if todo.iter().any(|(x, y)| x == t && y == v) {
                "fetch and verify"
            } else {
                "already here"
            };
            r = r.detail(format!("{t} {v}: {verb}"));
        }
        for w in &warnings {
            r = r.detail(w.clone());
        }
        return done(r.data(json!({
            "current": cur.to_string(), "target": tv.to_string(), "dry_run": true,
            "changes": plan_lines,
        })));
    }

    // fetch and verify only what is not here, outside the transaction
    let fetched = fetch_builds(cx, &m, &todo)?;

    if crate::advisories::visible(&cx.ui) {
        crate::advisories::mark(&mut cx.state, &shown.iter().collect::<Vec<_>>());
    }
    let mut txn = cx.begin(&format!("update carton {cur} to {tv}"))?;
    let txn_id = txn.id().to_string();
    let staged = stage(cx, &mut txn, &m, &target, &cur, &changes, &fetched, a);
    let staged = match staged {
        Ok(s) => s,
        Err(e) => return Err(fail(txn, e)),
    };

    if a.trial_only {
        abandon(txn)?;
        let mut r = Report::done(format!(
            "trial of carton {tv} against carton {cur}: nothing was switched or kept"
        ));
        for l in trial_lines(&staged.trial, true) {
            r = r.detail(l);
        }
        for l in &plan_lines {
            r = r.detail(l.clone());
        }
        let r = r
            .next(if staged.trial.different.is_empty() {
                "ciginstaller update"
            } else {
                "ciginstaller update --yes-differences"
            })
            .data(trial_json(&staged.trial));
        return done(r);
    }

    if let Verdict::Keep = decide(cx, &staged.trial, a, &tv) {
        let n = staged.trial.different.len();
        let names: Vec<String> = staged
            .trial
            .different
            .iter()
            .take(5)
            .map(|d| d.script.clone())
            .collect();
        let e = refused(
            "E1851",
            format!(
                "the trial found {n} of {} script(s) planning differently on carton {tv}; carton {cur} stays",
                staged.trial.compared
            ),
            "ciginstaller update --trial-only",
        )
        .with_hint(names.join(", "));
        return Err(fail(txn, e));
    }

    // switch: the new state, state.previous, the index flip
    let r = switch(cx, &mut txn, &m, &target, &cur, &fetched);
    settle(txn, r)?;
    let _ = prune_txns(
        &cx.home,
        cx.lock_ref()?,
        std::time::Duration::from_secs(7 * 24 * 3600),
    );
    for f in &fetched {
        let _ = std::fs::remove_file(&f.staged.path);
    }
    cx.state = State::load(&cx.home).map_err(ie)?;

    // a cig that changed says which of this machine's reported bugs the new release fixed; the
    // update is done, so nothing here can fail it
    let announced = match changes
        .iter()
        .find(|c| c.tool == "cig" && c.to.is_some() && c.from != c.to)
        .and_then(|c| c.to.clone())
    {
        Some(new_cig) => super::family::announce_fixed(&cx.home, &new_cig),
        None => Vec::new(),
    };

    // report, with the rollback hint
    let mut r = Report::done(format!("updated carton {cur} to {tv}")).detail(fresh.clone());
    for l in &announced {
        r = r.detail(l.clone());
    }
    for l in &links {
        r = r.detail(l.clone());
    }
    for l in &plan_lines {
        r = r.detail(l.clone());
    }
    for l in trial_lines(&staged.trial, false) {
        r = r.detail(l);
    }
    if fetched.is_empty() {
        r = r.detail("nothing needed fetching; the files were already here and intact");
    } else {
        let mut parts = Vec::new();
        for lvl in [
            TrustLevel::Pinned,
            TrustLevel::Attested,
            TrustLevel::Checksummed,
        ] {
            let n = fetched
                .iter()
                .filter(|f| f.staged.verified.level == lvl)
                .count();
            if n > 0 {
                parts.push(format!("{} {n}", lvl.as_str()));
            }
        }
        r = r.detail(format!("trust: {}", parts.join(", ")));
    }
    for w in &warnings {
        r = r.detail(w.clone());
    }
    r = r
        .detail(format!(
            "carton {cur} stays installed until the next update, so you can go back"
        ))
        .next("ciginstaller rollback");
    done(r.data(json!({
        "from": cur.to_string(), "to": tv.to_string(), "txn": txn_id,
        "changes": plan_lines, "whats_new": fresh, "links": links,
        "announced": announced,
        "fetched": fetched.iter().map(|f| json!({"tool": f.tool, "version": f.version.to_string()})).collect::<Vec<_>>(),
        "trial": trial_json(&staged.trial),
    })))
}

/// Roll the transaction back and give the error back (with `E1833` when the undo fell short).
fn fail(txn: Txn, e: InstallerError) -> InstallerError {
    settle::<()>(txn, Err(e))
        .err()
        .unwrap_or_else(|| bug("a failed update settled as a success"))
}

enum Pick {
    Target(Carton),
    /// Nothing to do: the report says so.
    Nothing(Report),
}

fn choose(m: &Manifest, cur: &Version, a: &UpdateArgs) -> Result<Pick, InstallerError> {
    let c = match &a.to {
        Some(want) => {
            let pin = Pin::parse(want).map_err(|e| {
                InstallerError::new("E1870", e.to_string())
                    .with_fix("ciginstaller carton list")
                    .with_exit(EXIT_USAGE)
            })?;
            let c = m.newest_matching(&pin, None).ok_or_else(|| {
                refused(
                    "E1811",
                    format!("there is no carton {want} on the {} channel", m.channel),
                    "ciginstaller carton list",
                )
            })?;
            if m.is_carton_yanked(&c.version) {
                let why = m
                    .yanked
                    .iter()
                    .find(|y| y.what == cig_compat::Yankable::Carton(c.version.clone()))
                    .map(|y| y.reason.clone())
                    .unwrap_or_default();
                return Err(refused(
                    "E1803",
                    format!("carton {} was yanked: {why}", c.version),
                    "ciginstaller carton list",
                ));
            }
            if c.version < *cur && !a.allow_downgrade {
                return Err(refused(
                    "E1850",
                    format!(
                        "carton {} is older than carton {cur}, which this machine is on",
                        c.version
                    ),
                    &format!("ciginstaller update --to {} --allow-downgrade", c.version),
                )
                .with_hint("`ciginstaller rollback` goes back one step without asking"));
            }
            if c.version == *cur {
                return Ok(Pick::Nothing(
                    Report::done(format!("this machine is already on carton {cur}"))
                        .data(json!({ "current": cur.to_string(), "newer": false })),
                ));
            }
            c
        }
        None => match m.latest(false) {
            Some(c) if c.version > *cur => c,
            _ => {
                return Ok(Pick::Nothing(
                    Report::done(format!(
                        "carton {cur} is the newest on the {} channel",
                        m.channel
                    ))
                    .data(json!({ "current": cur.to_string(), "newer": false })),
                ))
            }
        },
    };
    Ok(Pick::Target(c.clone()))
}

/// The tools whose build differs between the two cartons. Uses the manifest's `diff`, and the
/// record in `state.json` when the carton this machine is on is no longer in the manifest.
fn changes_of(m: &Manifest, cur: &Version, target: &Carton, state: &State) -> Vec<ToolChange> {
    if m.carton(cur).is_some() {
        return m
            .diff(cur, &target.version)
            .into_iter()
            .filter(|c| c.tool != "ciginstaller")
            .collect();
    }
    let have = state.cartons.get(&cur.to_string());
    let mut names: Vec<&String> = target.products.keys().collect();
    if let Some(r) = have {
        names.extend(r.tools.keys());
    }
    names.sort();
    names.dedup();
    names
        .into_iter()
        .filter(|t| t.as_str() != "ciginstaller")
        .filter_map(|t| {
            let from = have
                .and_then(|r| r.tools.get(t))
                .and_then(|v| Version::parse(v).ok());
            let to = target.products.get(t).cloned();
            (from != to).then(|| ToolChange {
                tool: t.clone(),
                from,
                to,
            })
        })
        .collect()
}

/// #7's links, as far as the manifest allows. The manifest has no per-tool changelog field, so the
/// closest honest form is each changed build's release page, built from the tool's own `repo` and
/// `tag` template (`https://github.com/<repo>/releases/tag/<tag>`).
fn whats_new_links(m: &Manifest, changes: &[ToolChange], host: &str) -> Vec<String> {
    changes
        .iter()
        .filter_map(|c| {
            let to = c.to.as_ref()?;
            let def = m.tool(&c.tool)?;
            let target = if def.targets.iter().any(|t| t == "any") {
                "any"
            } else {
                host
            };
            let names = def.names(to, target).ok()?;
            Some(format!(
                "{} {to}: https://github.com/{}/releases/tag/{}",
                c.tool, def.repo, names.tag
            ))
        })
        .collect()
}

/// #7's "what's new" line, in its words, dash included: `fresh carton: 1.2.0 (em dash) sealed
/// smoke log (cig), rehearse (cigo)`. The words are the carton's own `notes`; a carton with none
/// says which builds changed instead.
fn whats_new(target: &Carton, changes: &[ToolChange]) -> String {
    let notes = target.notes.trim();
    let body = if notes.is_empty() {
        let moved: Vec<String> = changes
            .iter()
            .filter_map(|c| c.to.as_ref().map(|to| format!("{} {to}", c.tool)))
            .collect();
        if moved.is_empty() {
            "no tool changed".to_string()
        } else {
            moved.join(", ")
        }
    } else {
        notes.to_string()
    };
    format!("fresh carton: {} \u{2014} {body}", target.version)
}

fn change_line(c: &ToolChange) -> String {
    match (c.kind(), &c.from, &c.to) {
        (ChangeKind::Added, _, Some(to)) => format!("{} {to} (new in this carton)", c.tool),
        (ChangeKind::Removed, Some(from), _) => {
            format!("{} {from} (not in this carton)", c.tool)
        }
        (ChangeKind::Downgraded, Some(f), Some(t)) => format!("{} {f} -> {t} (older)", c.tool),
        (_, Some(f), Some(t)) => format!("{} {f} -> {t}", c.tool),
        _ => c.tool.clone(),
    }
}

/// Refusals before anything is fetched, and the warnings of the marry-up check on the target.
fn plan_checks(
    cx: &Cx,
    m: &Manifest,
    cur: &Version,
    tv: &Version,
    changes: &[ToolChange],
    want: &[(String, Version)],
    read_only: bool,
) -> Result<(Vec<String>, Vec<cig_compat::Finding>), InstallerError> {
    let mut warnings = Vec::new();
    let mut shown: Vec<cig_compat::Finding> = Vec::new();
    let mut reminded = 0usize;
    for (t, v) in want {
        if m.is_product_yanked(t, v) {
            return Err(refused(
                "E1803",
                format!("{t} {v} was yanked"),
                "ciginstaller carton list",
            ));
        }
        if !present(&cx.home, t, v) && m.artifact(t, v, &cx.target).is_none() {
            return Err(refused(
                "E1815",
                format!("there is no build of {t} {v} for {}", cx.target),
                &format!("ciginstaller carton show {tv}"),
            ));
        }
    }
    for f in check(
        m,
        &Combination {
            items: want.to_vec(),
            origin: Origin::Carton(tv.clone()),
        },
        &cx.host,
    ) {
        if f.severity == Severity::Refuse {
            return Err(finding_error(&f));
        }
        if crate::advisories::is_advisory(&f) {
            match crate::advisories::triage(m, &cx.state, &f, read_only) {
                crate::advisories::Triage::Skip => continue,
                crate::advisories::Triage::Remind => {
                    reminded += 1;
                    continue;
                }
                crate::advisories::Triage::Show => shown.push(f.clone()),
            }
        }
        warnings.push(format!("warning {}", crate::advisories::line(&f)));
    }
    if reminded > 0 {
        warnings.push(format!("warning {}", crate::advisories::reminder(reminded)));
    }
    for c in changes.iter().filter(|c| c.kind() == ChangeKind::Removed) {
        warnings.push(format!(
            "warning: carton {tv} has no {t}, so its shim has nothing to run after the switch; \
             keep it with `ciginstaller install stick {t} --carton {cur}`",
            t = c.tool
        ));
    }
    Ok((warnings, shown))
}

/// E1854 as a note: a project that pins a carton the target does not satisfy keeps running its
/// own; the machine carton still moves.
fn project_note(cx: &Cx, a: &UpdateArgs, tv: &Version) -> Option<String> {
    let dir = a.project.clone().unwrap_or_else(|| cx.cwd.clone());
    let p = project::discover(&dir, &cx.home.root(), cigi_shim::user_home().as_deref())
        .ok()
        .flatten()?;
    let (pin, file) = p.carton_pin?;
    if pin.satisfied_by(tv) {
        return None;
    }
    Some(format!(
        "note E1854: {} pins carton {pin}, which carton {tv} does not satisfy; \
         that project keeps running its pinned carton, and the machine carton still moves",
        file.display()
    ))
}

// ------------------------------------------------------------------------------ staging

struct Staged {
    trial: TrialOutcome,
}

/// Inside the transaction, before the switch: materialize the new toolchains beside the old, then
/// the trial. Nothing here changes what runs.
#[allow(clippy::too_many_arguments)]
fn stage(
    cx: &Cx,
    txn: &mut Txn,
    m: &Manifest,
    target: &Carton,
    cur: &Version,
    changes: &[ToolChange],
    fetched: &[Fetched],
    a: &UpdateArgs,
) -> Result<Staged, InstallerError> {
    materialize_fetched(txn, &cx.home, m, &target.version, fetched)?;
    if a.no_trial {
        cx.note("trial skipped (--no-trial)");
        return Ok(Staged {
            trial: TrialOutcome {
                skipped: vec!["trial skipped (--no-trial)".to_string()],
                ..TrialOutcome::default()
            },
        });
    }
    let trials = trial::registered(trusted_for(&cx.home.root()));
    let mut pairs: Vec<(Resolved, Resolved)> = Vec::new();
    let mut skipped = Vec::new();
    for c in changes {
        let (Some(from), Some(to)) = (&c.from, &c.to) else {
            continue;
        };
        if !trials.iter().any(|t| t.tool() == c.tool) {
            continue;
        }
        let side = |v: &Version, carton: &Version| {
            tool_exe(&cx.home, &c.tool, v).map(|path| Resolved {
                tool: c.tool.clone(),
                carton: carton.to_string(),
                version: v.clone(),
                path,
            })
        };
        match (side(from, cur), side(to, &target.version)) {
            (Some(o), Some(n)) => pairs.push((o, n)),
            _ => skipped.push(format!(
                "{}: the old or the new build has no readable receipt",
                c.tool
            )),
        }
    }
    if pairs.is_empty() {
        cx.note("trial skipped: no tool with a trial changed");
        let mut t = TrialOutcome::default();
        t.skipped.push("no tool with a trial changed".to_string());
        t.skipped.extend(skipped);
        return Ok(Staged { trial: t });
    }
    let Some(ctx) = trial_project(cx, a.project.as_deref())? else {
        cx.note("no project here, trial skipped");
        return Ok(Staged {
            trial: TrialOutcome {
                skipped: vec!["no project here".to_string()],
                ..TrialOutcome::default()
            },
        });
    };
    cx.note(&format!("trial on {}", ctx.root.display()));
    let mut out = trial::run_all(&trials, &pairs, &ctx, trial::TOTAL_BUDGET);
    out.skipped.extend(skipped);
    Ok(Staged { trial: out })
}

/// The project in scope for the trial: `--project <dir>`, else the nearest directory upward from the
/// working directory holding `cig-tasks.json`, `.cig-carton` or a `*.cig` file. The search stops at
/// the user's home, at the installer's own directory and at a shared (world-writable or sticky)
/// directory, and uses only files this user owns.
fn trial_project(cx: &Cx, explicit: Option<&Path>) -> Result<Option<ProjectCtx>, InstallerError> {
    if let Some(dir) = explicit {
        let root = std::fs::canonicalize(dir).map_err(|e| {
            InstallerError::new("E1870", format!("--project {}: {e}", dir.display()))
                .with_fix("ciginstaller update --project .")
                .with_exit(EXIT_USAGE)
        })?;
        if !root.is_dir() {
            return Err(InstallerError::new(
                "E1870",
                format!("--project {} is not a directory", dir.display()),
            )
            .with_fix("ciginstaller update --project .")
            .with_exit(EXIT_USAGE));
        }
        // the same footing as the automatic search: a directory the installer home's owner owns
        // (or root), and not a shared one
        if !trusted_entry(&root, trusted_for(&cx.home.root()), true) || is_shared(&root) {
            return Err(InstallerError::new(
                "E1870",
                format!(
                    "--project {} is shared or belongs to another user; the trial only runs scripts of this user's own projects",
                    dir.display()
                ),
            )
            .with_fix("ciginstaller update --no-trial")
            .with_exit(EXIT_USAGE));
        }
        return Ok(Some(ProjectCtx {
            root,
            ..ProjectCtx::default()
        }));
    }
    let home_root = cx.home.root();
    let user_home = cigi_shim::user_home();
    let trust = trusted_for(&home_root);
    let mut dir = cx.cwd.clone();
    for _ in 0..40 {
        if Some(dir.as_path()) == user_home.as_deref() || dir == home_root || is_shared(&dir) {
            return Ok(None);
        }
        if trusted_entry(&dir, trust, true) && has_marker(&dir, trust) {
            return Ok(Some(ProjectCtx {
                root: dir,
                ..ProjectCtx::default()
            }));
        }
        if !dir.pop() {
            break;
        }
    }
    Ok(None)
}

fn has_marker(dir: &Path, trust: cigi_shim::Trusted) -> bool {
    if ["cig-tasks.json", ".cig-carton"]
        .iter()
        .any(|f| trusted_entry(&dir.join(f), trust, false))
    {
        return true;
    }
    std::fs::read_dir(dir).is_ok_and(|rd| {
        rd.flatten().any(|e| {
            e.file_name().to_string_lossy().ends_with(".cig")
                && trusted_entry(&e.path(), trust, false)
        })
    })
}

#[cfg(unix)]
fn is_shared(dir: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    // world-writable or sticky: /tmp and the like, never a project
    std::fs::metadata(dir).is_ok_and(|m| m.permissions().mode() & 0o1002 != 0)
}
#[cfg(not(unix))]
fn is_shared(_dir: &Path) -> bool {
    false
}

// -------------------------------------------------------------------------- the decision

enum Verdict {
    Accept,
    Keep,
}

/// What to do with a trial that found differences (SPEC 12.2 step 6): `--yes-differences` accepts;
/// `--yes` accepts identical results only; on a terminal ask `[a]ccept [r]eview [k]eep` (keep is
/// the default); with no terminal keep, and say so.
fn decide(cx: &Cx, t: &TrialOutcome, a: &UpdateArgs, tv: &Version) -> Verdict {
    if t.different.is_empty() {
        return Verdict::Accept;
    }
    if a.yes_differences {
        return Verdict::Accept;
    }
    if cx.ui.yes {
        cx.note(
            "--yes accepts identical trial results only; add --yes-differences to accept these",
        );
        return Verdict::Keep;
    }
    let mut q = Question::yes_no(
        &format!("The trial found differences on carton {tv}. Switch anyway?"),
        false,
    )
    .summary(&format!(
        "{} of {} script(s) plan differently",
        t.different.len(),
        t.compared
    ));
    for d in t.different.iter().take(8) {
        q = q.detail(&format!("{}: {}", d.script, d.what));
    }
    q.choices = vec![
        Choice::new('a', "accept"),
        Choice::new('r', "review"),
        Choice {
            refusal: true,
            ..Choice::new('k', "keep")
        },
    ];
    q.default = 2;
    for _ in 0..3 {
        let ans = cx.ui.ask(&q);
        match ans.key {
            'a' => return Verdict::Accept,
            'r' => {
                for d in &t.different {
                    let mut r = Report::new(Icon::Note, format!("{}: {}", d.script, d.what));
                    for l in d.diff.lines() {
                        r = r.detail(l.to_string());
                    }
                    cx.ui.say(&r);
                }
            }
            _ => return Verdict::Keep,
        }
    }
    Verdict::Keep
}

fn trial_lines(t: &TrialOutcome, full: bool) -> Vec<String> {
    let mut v = Vec::new();
    if t.compared > 0 {
        v.push(format!(
            "trial: {} script(s) compared, {} the same, {} different",
            t.compared,
            t.same,
            t.different.len()
        ));
    }
    for d in &t.different {
        v.push(format!("different: {}: {}", d.script, d.what));
        if full {
            v.extend(d.diff.lines().map(|l| format!("  {l}")));
        }
    }
    for s in &t.skipped {
        v.push(format!("trial skipped: {s}"));
    }
    v
}

fn trial_json(t: &TrialOutcome) -> serde_json::Value {
    json!({
        "compared": t.compared, "same": t.same, "skipped": t.skipped,
        "different": t.different.iter().map(|d| json!({
            "script": d.script, "what": d.what, "diff": d.diff,
        })).collect::<Vec<_>>(),
    })
}

/// Roll the transaction back on purpose (`--trial-only`): the home is as it was.
fn abandon(txn: Txn) -> Result<(), InstallerError> {
    match txn.rollback() {
        Ok(rep) if rep.is_complete() => Ok(()),
        Ok(rep) => Err(InstallerError::new(
            "E1833",
            format!(
                "the trial's scratch work could not be fully undone: {} step(s) failed; the next command finishes it",
                rep.failed.len()
            ),
        )
        .with_fix("ciginstaller doctor")),
        Err(e) => Err(ie(e)),
    }
}

// ------------------------------------------------------------------------------ the switch

/// New state, `state.previous`, shims and the index flip (the caller commits).
fn switch(
    cx: &Cx,
    txn: &mut Txn,
    m: &Manifest,
    target: &Carton,
    cur: &Version,
    fetched: &[Fetched],
) -> Result<(), InstallerError> {
    let home = cx.home.clone();
    let before = cx.state.clone();
    let mut state = before.clone();
    let now = now_rfc3339();
    let tv = target.version.to_string();
    let curs = cur.to_string();
    let newly_added = !state.cartons.contains_key(&tv);
    let mut tools = std::collections::BTreeMap::new();
    for (t, v) in target.products.iter().filter(|(t, _)| *t != "ciginstaller") {
        // the receipt must be there: every tool of the carton is installed by now
        if read_receipt(&home, t, v).is_err() {
            return Err(bug(format!(
                "{t} {v} has no receipt after it was installed"
            )));
        }
        tools.insert(t.clone(), v.to_string());
    }
    state.cartons.insert(
        tv.clone(),
        CartonRecord {
            installed_at: state
                .cartons
                .get(&tv)
                .map(|r| r.installed_at.clone())
                .unwrap_or_else(|| now.clone()),
            tools,
            reason: "update".to_string(),
            last_used: state.cartons.get(&tv).and_then(|r| r.last_used.clone()),
            partial: false,
        },
    );
    let old_previous = before.previous.clone();
    state.machine_carton = Some(tv.clone());
    state.previous = Previous {
        machine_carton: Some(curs.clone()),
        kept_until: None,
    };
    // a tool rolled back on its own was a choice about the old carton
    state.sticks.retain(|s| !s.is_rollback());
    if newly_added {
        let undo = UpdateUndo {
            added: tv.clone(),
            from: curs.clone(),
            previous: old_previous,
            txn: txn.id().to_string(),
        };
        state.extra.insert(UNDO_KEY.to_string(), undo.to_value());
    } else {
        state.extra.remove(UNDO_KEY);
    }
    state.channel = cx.channel();
    let lowest = fetched
        .iter()
        .map(|f| f.staged.verified.level)
        .min()
        .map(|l| l.as_str().to_string())
        .or_else(|| state.manifest.as_ref().map(|x| x.trust_level.clone()))
        .unwrap_or_else(|| "checksummed".to_string());
    state.manifest = Some(ManifestInfo {
        sequence: m.sequence,
        channel: m.channel.clone(),
        fetched_at: now.clone(),
        expires: m.expires.clone(),
        trust_level: lowest,
    });
    cx.record_manifest_digest(&mut state, &m.channel, m.sequence);
    state.push_history(HistoryEntry {
        id: txn.id().to_string(),
        at: now,
        what: "update".into(),
        from: Some(curs),
        to: Some(tv),
        txn: txn.id().to_string(),
        ok: true,
    });
    resync_shims(txn, &home, &before, &state)?;
    save_and_flip(txn, &home, &state, yanked_in(m))
}
