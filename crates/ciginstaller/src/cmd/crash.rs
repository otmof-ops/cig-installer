// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `crash`: the family ashtray (#10, #28 part 1). `list`, `show`, `send`, `delete` over every
//! product's reports, and `submit` for a product handing one in (a superset of
//! `cig crash submit`).
//!
//! The commands do not need the installer's state (they are the way to look at a crash when
//! that state is the problem), so they read the home and the config and nothing else.
//!
//! Nothing leaves this machine without a yes. A report is sent only by `send`, which is the yes
//! for the report it names, and a `never` anywhere (`CIG_REPORTS`, `CIGI_REPORTS`, the `reports`
//! setting) is a refusal even then. Sending is one request, of exactly what `show` prints, to
//! the issues route; where no route exists yet the report is kept and the output says so.

use std::path::{Path, PathBuf};

use cigi_home::{Config, Home};
use cigi_ui::{Align, Cell, Icon, InstallerError, Question, Report, Ui, EXIT_USAGE};
use serde_json::{json, Value};

use super::tabular::details;
use crate::cli::{CrashCmd, CrashSub};
use crate::crash::{self, anon, route, Consent, Found, Kind, Report as Saved, Source};
use crate::errors::ie;

pub struct Env<'a> {
    pub ui: &'a Ui,
    pub root: PathBuf,
    pub cfg: Config,
}

/// The home and the config, and nothing else: no installer state, so it works when that is
/// the problem. `report --send` goes through the same door.
pub fn open_env<'a>(ui: &'a Ui, home_flag: Option<&PathBuf>) -> Result<Env<'a>, InstallerError> {
    let over = home_flag.map(|h| h.clone().into_os_string());
    let home = Home::discover_with(&|k| match (k, &over) {
        ("CIGSCRIPT_HOME", Some(h)) => Some(h.clone()),
        _ => std::env::var_os(k),
    })
    .map_err(ie)?;
    let cfg = Config::load(&home).map_err(ie)?;
    Ok(Env {
        ui,
        root: home.root(),
        cfg,
    })
}

pub fn run(ui: &Ui, home_flag: Option<&PathBuf>, cmd: &CrashCmd) -> Result<Report, InstallerError> {
    let env = open_env(ui, home_flag)?;
    match cmd.sub.as_ref().unwrap_or(&CrashSub::List) {
        CrashSub::List => list(&env),
        CrashSub::Show { id } => show(&env, id),
        CrashSub::Send { id, all } => send(&env, id.as_deref(), *all),
        CrashSub::Delete { id, all } => delete(&env, id.as_deref(), *all),
        CrashSub::Submit {
            origin,
            file,
            repo,
            now,
        } => submit(&env, origin, file, repo.as_deref(), *now),
    }
}

fn usage(msg: impl Into<String>) -> InstallerError {
    InstallerError::new("E1870", msg).with_exit(EXIT_USAGE)
}

fn find(env: &Env, id: &str) -> Result<Saved, InstallerError> {
    match crash::find(&env.root, id)
        .map_err(|e| usage(format!("the ashtray cannot be read: {e}")))?
    {
        Found::One(r) => Ok(*r),
        Found::None => {
            Err(usage(format!("no report matches `{id}`")).with_fix("ciginstaller crash list"))
        }
        Found::Many(n) => Err(
            usage(format!("`{id}` matches {n} reports; give more of the id"))
                .with_fix("ciginstaller crash list"),
        ),
    }
}

fn consent_word(c: Consent) -> &'static str {
    match c {
        Consent::Ask => "ask",
        Consent::Always => "always",
        Consent::Never => "never",
    }
}

fn route_state() -> Value {
    match route::route_from_env() {
        Ok(Some(u)) => json!({ "configured": true, "host": host_of(&u) }),
        Ok(None) => json!({ "configured": false, "host": null }),
        Err(e) => json!({ "configured": false, "host": null, "problem": e }),
    }
}

fn host_of(u: &url::Url) -> String {
    match u.port() {
        Some(p) => format!("{}:{p}", u.host_str().unwrap_or("?")),
        None => u.host_str().unwrap_or("?").to_string(),
    }
}

// ----- list -------------------------------------------------------------------------------

fn list(env: &Env) -> Result<Report, InstallerError> {
    let l =
        crash::list(&env.root).map_err(|e| usage(format!("the ashtray cannot be read: {e}")))?;
    let (consent, source) = crash::consent(&env.cfg);
    let waiting = l.reports.iter().filter(|r| !r.filed()).count();
    let data = json!({
        "reports": l.reports.iter().map(|r| json!({
            "id": r.id, "origin": r.origin, "kind": r.kind.word(), "version": r.version,
            "state": r.state(), "title": r.headline(), "fingerprint": r.fingerprint,
            "target": r.target(), "issue": r.issue.as_ref().map(|i| i.url.clone()),
            "attempts": r.queue.attempts, "last_error": r.queue.last_error,
        })).collect::<Vec<_>>(),
        "waiting": waiting,
        "unreadable": l.unreadable,
        "reports_setting": consent_word(consent),
        "route": route_state(),
    });
    if l.reports.is_empty() {
        let mut r = Report::done("the ashtray is empty: no crash reports (that is good)");
        if l.unreadable > 0 {
            r = r.detail(format!(
                "{} file(s) in it are not reports this build can read; they are left alone",
                l.unreadable
            ));
        }
        return Ok(r.data(data));
    }
    let mut table = cigi_ui::Table::new(&[
        ("report", Align::Left),
        ("origin", Align::Left),
        ("kind", Align::Left),
        ("state", Align::Left),
        ("what", Align::Left),
    ]);
    for r in &l.reports {
        let state = r.state();
        table = table.row(vec![
            Cell::new(r.id.clone()),
            Cell::new(r.origin.clone()),
            Cell::new(r.kind.word()),
            if state.starts_with("failed") {
                Cell::changed(crash::truncate(&state, 30))
            } else {
                Cell::new(state)
            },
            Cell::new(crash::truncate(r.headline(), 60)),
        ]);
    }
    let mut rep = Report::done(format!(
        "{} report(s) from every tool, {waiting} not filed; reports = {} ({})",
        l.reports.len(),
        consent_word(consent),
        match source {
            Source::CigReports => "CIG_REPORTS",
            Source::Setting => "set",
            Source::Ci => "CI default",
            Source::Default => "default",
        }
    ));
    for line in details(env.ui, &table) {
        rep = rep.detail(line);
    }
    if l.unreadable > 0 {
        rep = rep.detail(format!(
            "{} file(s) in the ashtray are not reports this build can read; they are left alone",
            l.unreadable
        ));
    }
    Ok(rep.next("ciginstaller crash show <report>").data(data))
}

// ----- show -------------------------------------------------------------------------------

fn show(env: &Env, id: &str) -> Result<Report, InstallerError> {
    let r = find(env, id)?;
    let view = route::travel(&r, &env.root);
    let mut rep = Report::done(format!(
        "{} report {} from {}",
        r.kind.word(),
        r.id,
        r.origin
    ));
    if env.ui.mode != cigi_ui::Mode::Json {
        rep = rep.detail(format!(
            "this is exactly what `ciginstaller crash send {}` sends, to the issues route for {}:",
            r.id,
            r.target()
        ));
        for l in serde_json::to_string_pretty(&view)
            .unwrap_or_default()
            .lines()
        {
            rep = rep.detail(l.to_string());
        }
        if let Some(x) = view.get("repro").and_then(|x| x["script"].as_str()) {
            rep = rep.detail(String::new()).detail("repro:".to_string());
            for l in x.lines() {
                rep = rep.detail(l.to_string());
            }
        }
    }
    Ok(rep
        .next(format!("ciginstaller crash send {}", r.id))
        .data(json!({ "id": r.id, "state": r.state(), "target": r.target(), "report": view })))
}

// ----- send -------------------------------------------------------------------------------

pub fn never_error(env: &Env, r: Option<&Saved>) -> InstallerError {
    let fix = if std::env::var_os("CIG_REPORTS").is_some() {
        "unset CIG_REPORTS (or set it to ask), then `ciginstaller crash send` again"
    } else {
        "ciginstaller config set reports ask"
    };
    let kept = r.map_or_else(String::new, |r| {
        format!(
            "; the report stays at {}",
            crash::path_of(&env.root, r).display()
        )
    });
    InstallerError::new(
        "E1857",
        format!("reports is \"never\", so nothing leaves this machine{kept}"),
    )
    .with_fix(fix)
    .with_exit(cigi_ui::EXIT_REFUSED)
}

enum Sent {
    Filed(Option<String>),
    /// Nothing to send to yet; the report is kept.
    NoRoute,
    AlreadyFiled(Option<String>),
}

fn issue_of(url: &str) -> Option<crash::Issue> {
    let u = url::Url::parse(url).ok()?;
    let parts: Vec<&str> = u.path_segments()?.collect();
    let i = parts.iter().position(|p| *p == "issues")?;
    let number = parts.get(i + 1)?.parse().ok()?;
    (i >= 2).then(|| crash::Issue {
        repo: format!("{}/{}", parts[i - 2], parts[i - 1]),
        number,
        url: url.to_string(),
    })
}

fn send_one(env: &Env, r: &mut Saved) -> Result<Sent, InstallerError> {
    if r.filed() {
        return Ok(Sent::AlreadyFiled(r.issue.as_ref().map(|i| i.url.clone())));
    }
    if crash::explicit_never(&env.cfg) {
        return Err(never_error(env, Some(r)));
    }
    let route = route::route_from_env().map_err(|e| {
        InstallerError::new("E1857", format!("the issues route cannot be used: {e}"))
            .with_fix("ciginstaller crash show")
    })?;
    r.queue.requested = true;
    let Some(route) = route else {
        let _ = crash::write(&env.root, r);
        return Ok(Sent::NoRoute);
    };
    let body = route::travel(r, &env.root);
    match route::post(&route, &body) {
        Ok(url) => {
            r.sent = Some(cigi_home::now_rfc3339());
            r.issue = url.as_deref().and_then(issue_of);
            r.queue.last_error = None;
            r.queue.attempts += 1;
            let _ = crash::write(&env.root, r);
            Ok(Sent::Filed(url))
        }
        Err(f) => {
            r.queue.attempts += 1;
            r.queue.last_error = Some(f.why.clone());
            let _ = crash::write(&env.root, r);
            Err(InstallerError::new(
                "E1857",
                format!(
                    "could not send {} to {}: {}; it stays queued at {}",
                    r.id,
                    host_of(&route),
                    f.why,
                    crash::path_of(&env.root, r).display()
                ),
            )
            .with_fix(format!("ciginstaller crash send {}", r.id))
            .with_subject(r.id.clone()))
        }
    }
}

fn no_route_report(env: &Env, ids: &[String]) -> Report {
    let kept = ids.len();
    Report::new(
        Icon::Note,
        if kept == 1 {
            format!("nothing was sent: there is no issues route yet; {} stays queued here", ids[0])
        } else {
            format!("nothing was sent: there is no issues route yet; {kept} reports stay queued here")
        },
    )
    .detail("the relay and the scoped token that file a report without a person's own login are still to come (#28), and the courier that sends a queued report even after a crash is #24")
    .detail(format!("the report(s) wait in {}; `ciginstaller crash show <report>` shows what would be sent", crash::ashtray_dir(&env.root).display()))
    .detail(format!("a route you run yourself: set {} to its https:// address", route::ROUTE_ENV))
    .data(json!({ "sent": [], "queued": ids, "route": { "configured": false } }))
}

pub fn send(env: &Env, id: Option<&str>, all: bool) -> Result<Report, InstallerError> {
    let mut targets: Vec<Saved> = match (id, all) {
        (Some(id), _) => vec![find(env, id)?],
        (None, true) => {
            let mut v = crash::list(&env.root)
                .map_err(|e| usage(format!("the ashtray cannot be read: {e}")))?
                .reports;
            v.retain(|r| !r.filed());
            v.reverse();
            v
        }
        (None, false) => {
            return Err(usage("give a report id, or --all").with_fix("ciginstaller crash list"))
        }
    };
    if targets.is_empty() {
        return Ok(
            Report::done("nothing is waiting to be sent").data(json!({ "sent": [], "queued": [] }))
        );
    }
    let mut sent = Vec::new();
    let mut queued = Vec::new();
    let mut failed: Vec<(String, String)> = Vec::new();
    let mut known = Vec::new();
    for r in &mut targets {
        match send_one(env, r) {
            Ok(Sent::Filed(url)) => sent.push(json!({ "id": r.id, "url": url })),
            Ok(Sent::AlreadyFiled(url)) => known.push(json!({ "id": r.id, "url": url })),
            Ok(Sent::NoRoute) => queued.push(r.id.clone()),
            Err(e) if id.is_some() => return Err(e),
            Err(e) if e.code == "E1857" && e.message.starts_with("reports is") => return Err(e),
            Err(e) => failed.push((r.id.clone(), e.message)),
        }
    }
    if !failed.is_empty() {
        let first = failed[0].1.clone();
        return Err(InstallerError::new(
            "E1857",
            format!("{} sent, {} stay queued: {first}", sent.len(), failed.len()),
        )
        .with_fix("ciginstaller crash send --all"));
    }
    if !queued.is_empty() && sent.is_empty() {
        return Ok(no_route_report(env, &queued));
    }
    let mut rep = Report::done(match (sent.len(), known.len()) {
        (1, 0) => format!("sent {}", sent[0]["id"].as_str().unwrap_or("")),
        (n, 0) => format!("sent {n} reports"),
        (_, k) => format!("sent {}; {k} already filed", sent.len()),
    });
    for s in sent.iter().chain(known.iter()) {
        if let Some(u) = s["url"].as_str() {
            rep = rep.detail(format!("{} is at {u}", s["id"].as_str().unwrap_or("")));
        }
    }
    Ok(rep.data(json!({ "sent": sent, "already_filed": known, "queued": queued })))
}

// ----- delete -----------------------------------------------------------------------------

fn delete(env: &Env, id: Option<&str>, all: bool) -> Result<Report, InstallerError> {
    if all {
        let l = crash::list(&env.root)
            .map_err(|e| usage(format!("the ashtray cannot be read: {e}")))?;
        if l.reports.is_empty() {
            return Ok(Report::done("nothing to delete").data(json!({ "deleted": [] })));
        }
        let q = Question::irreversible(&format!("delete all {} saved report(s)?", l.reports.len()))
            .summary("it cannot be undone; a report not yet filed is lost");
        let ans = env.ui.ask(&q);
        if !ans.is_yes(&q) {
            if ans.exit_code().is_some() {
                return Err(InstallerError::new(
                    "E1871",
                    "delete --all needs a yes and there is no terminal to ask on",
                )
                .with_fix("ciginstaller crash delete --all --yes-irreversible")
                .with_exit(EXIT_USAGE));
            }
            return Ok(
                Report::new(Icon::Note, "nothing was deleted").data(json!({ "deleted": [] }))
            );
        }
        let mut gone = Vec::new();
        for r in &l.reports {
            if std::fs::remove_file(crash::path_of(&env.root, r)).is_ok() {
                gone.push(r.id.clone());
            }
        }
        return Ok(Report::done(format!("deleted {} report(s)", gone.len()))
            .data(json!({ "deleted": gone })));
    }
    let Some(id) = id else {
        return Err(usage("give a report id, or --all").with_fix("ciginstaller crash list"));
    };
    let r = find(env, id)?;
    std::fs::remove_file(crash::path_of(&env.root, &r))
        .map_err(|e| InstallerError::new("E1857", format!("{} could not be deleted: {e}", r.id)))?;
    Ok(Report::done(format!("deleted {}", r.id)).data(json!({ "deleted": [r.id] })))
}

/// Keys of a handed-in report that are read into the report's own fields, or that must never
/// travel: they are not repeated in `details`.
const HANDLED: &[&str] = &[
    "own_paths",
    "id",
    "fingerprint",
    "kind",
    "origin",
    "version",
    "os",
    "arch",
    "timestamp",
    "command",
    "cwd",
    "panic_message",
    "panic_location",
    "message",
    "location",
    "title",
    "backtrace",
    "active_run_id",
    "rolled_back",
    "repro",
    "env",
    "environ",
    "environment",
    "envs",
    "variables",
    "vars",
    "secrets",
    "credentials",
];

// ----- submit -----------------------------------------------------------------------------

fn submit(
    env: &Env,
    origin: &str,
    file: &Path,
    repo: Option<&str>,
    now: bool,
) -> Result<Report, InstallerError> {
    let text = std::fs::read_to_string(file)
        .map_err(|e| usage(format!("cannot read {}: {e}", file.display())))?;
    let v: Value = serde_json::from_str(&text)
        .map_err(|e| usage(format!("{} is not JSON: {e}", file.display())))?;
    let origin = origin.trim();
    if origin.is_empty()
        || !origin
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(usage("--origin names the tool, like cig-orchestrator"));
    }
    if let Some(r) = repo {
        if r.split('/').count() != 2
            || r.contains(char::is_whitespace)
            || r.split('/').any(str::is_empty)
        {
            return Err(usage(format!(
                "--repo must look like owner/name, not `{r}`"
            )));
        }
    }
    let root = Some(env.root.as_path());
    // the paths the tool says are its own stay readable in what it handed in
    anon::know_words(crate::crash::hook::cli_words());
    if let Some(own) = v["own_paths"].as_array() {
        anon::register_own_paths(own.iter().filter_map(|p| p.as_str().map(str::to_string)));
    }
    let message = anon::anonymise_text_in(
        v["panic_message"]
            .as_str()
            .or_else(|| v["title"].as_str())
            .or_else(|| v["message"].as_str())
            .unwrap_or("(no message)"),
        root,
    );
    let location_raw = v["panic_location"]
        .as_str()
        .or_else(|| v["location"].as_str())
        .unwrap_or("?");
    let location = anon::scrub_text(location_raw, root);
    let kind = Kind::parse(v["kind"].as_str().unwrap_or("crash"));
    let tool_version = v["version"].as_str().unwrap_or("0");
    let tool_version: String = tool_version
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'))
        .take(40)
        .collect();
    // the fingerprint is the family's: the tool's own product and major version
    let mut r = crash::new_report(
        origin,
        &tool_version,
        kind,
        &location,
        &message,
        Vec::new(),
        root,
    );
    r.repo = repo.map(str::to_string);
    if let Some(c) = v["command"].as_array() {
        r.command = anon::redact_command(c.iter().filter_map(|a| a.as_str().map(str::to_string)));
    } else {
        r.command = Vec::new();
    }
    if let Some(cwd) = v["cwd"].as_str() {
        r.cwd = anon::shape(Path::new(cwd), root);
    } else {
        r.cwd = String::new();
    }
    if let Some(bt) = v["backtrace"].as_array() {
        r.backtrace = bt
            .iter()
            .filter_map(Value::as_str)
            .take(40)
            .map(|f| anon::scrub_text(f, root))
            .collect();
    }
    r.os = v["os"].as_str().map_or(r.os, |s| {
        s.chars()
            .filter(char::is_ascii_alphanumeric)
            .take(40)
            .collect()
    });
    r.arch = v["arch"].as_str().map_or(r.arch, |s| {
        s.chars()
            .filter(char::is_ascii_alphanumeric)
            .take(40)
            .collect()
    });
    r.active_run_id = v["active_run_id"].as_str().map(|s| {
        s.chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
            .take(64)
            .collect()
    });
    r.rolled_back = v["rolled_back"].as_bool();
    if v["version"].is_string() {
        r.title = Some(format!(
            "{} ({origin} {})",
            crash::truncate(&message, 400),
            r.version
        ));
    }
    if let Some(x) = v.get("repro").filter(|x| x.is_object()) {
        r.repro = Some(x.clone());
    }
    // what the report keeps in its own fields does not travel again as detail, and a tool's
    // environment never travels at all (#28: no environment values)
    let mut details = v.clone();
    if let Some(o) = details.as_object_mut() {
        o.retain(|k, _| !HANDLED.contains(&k.to_ascii_lowercase().as_str()));
    }
    anon::anonymise_value(&mut details, root);
    r.details = Some(details);
    crash::write_new(&env.root, &mut r)
        .map_err(|e| usage(format!("could not save the report: {e}")))?;
    let (consent, _) = crash::consent(&env.cfg);
    let id = r.id.clone();
    let mut rep = match consent {
        Consent::Never => Report::done(format!(
            "saved as {id} (reports is \"never\": it stays local)"
        )),
        Consent::Always => {
            r.queue.requested = true;
            let _ = crash::write(&env.root, &r);
            Report::done(format!(
                "saved as {id}; reports is \"always\", so it is queued"
            ))
        }
        Consent::Ask => Report::done(format!("saved as {id}")),
    };
    rep = rep.next(format!("ciginstaller crash show {id}"));
    if now {
        return match send_one(env, &mut r) {
            Ok(Sent::Filed(url)) => Ok(Report::done(format!(
                "saved as {id} and sent{}",
                url.map(|u| format!(": {u}")).unwrap_or_default()
            ))
            .data(json!({ "id": id, "sent": true }))),
            Ok(Sent::NoRoute) => Ok(no_route_report(env, std::slice::from_ref(&id))),
            Ok(Sent::AlreadyFiled(_)) => Ok(rep.data(json!({ "id": id, "sent": true }))),
            Err(e) => Err(e),
        };
    }
    Ok(
        rep.data(
            json!({ "id": id, "sent": false, "origin": origin, "fingerprint": r.fingerprint }),
        ),
    )
}
