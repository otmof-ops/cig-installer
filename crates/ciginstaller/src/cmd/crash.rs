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
use crate::crash::{
    self, anon, blame, health, route, routes, Consent, Found, Kind, Report as Saved, Source,
};
use crate::cx::{Cx, Globals};
use crate::errors::ie;
use crate::flows::report as report_flow;

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
        CrashSub::Send { id, all, via, out } => {
            let via = Via::parse(via.as_deref(), out.as_deref())?;
            send_via(&env, id.as_deref(), *all, &via)
        }
        CrashSub::TestRoute { repo } => test_route(&env, repo.as_deref()),
        CrashSub::Status => status(&env),
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
            "target": target_of(&env.root, r), "issue": r.issue.as_ref().map(|i| i.url.clone()),
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
            Source::Locked => "locked by the machine",
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

// ----- what travels ---------------------------------------------------------------------

/// The object `show` prints and `send` sends: `route::travel`'s, after the installer's version
/// block is attached to a report whose details carry none (#11). A report another tool queued
/// (cig, cigo, cigcompile, cigar) knows only its own version; the installer knows the carton and
/// every tool's build, so it adds that when the report leaves, marked as taken at send time.
/// Both commands come through here, so what `show` prints is byte for byte what `send` sends.
fn travelling(env: &Env, r: &Saved) -> Value {
    let has_block = r.details.as_ref().is_some_and(|d| {
        // `bug` keeps the block as `version_matrix`, `report`'s bundle as `version_block`
        ["version_matrix", "version_block"]
            .iter()
            .any(|k| d.get(k).is_some_and(|m| !m.is_null()))
    });
    let attachable = match &r.details {
        None => true,
        Some(d) => d.is_object() || d.is_null(),
    };
    if has_block || !attachable {
        return route::travel(r, &env.root);
    }
    let mut block = block_now(env);
    if let Some(o) = block.as_object_mut() {
        o.insert("taken".into(), json!("at send time"));
    }
    let mut with = r.clone();
    let mut d = match with.details.take() {
        Some(Value::Object(o)) => o,
        _ => serde_json::Map::new(),
    };
    d.insert("version_matrix".into(), block);
    with.details = Some(Value::Object(d));
    route::travel(&with, &env.root)
}

/// `ciginstaller report --block --json`'s block, read now. With no readable state it is the
/// reduced block that says so (the ashtray works when the state is the problem).
fn block_now(env: &Env) -> Value {
    let globals = Globals {
        yes: false,
        offline: true,
        home: Some(env.root.clone()),
        self_updated: None,
    };
    match Cx::new(env.ui.duplicate(), globals, None) {
        Ok(mut cx) => report_flow::version_block(&mut cx),
        Err(e) => report_flow::block_without_state(&e),
    }
}

// ----- show -------------------------------------------------------------------------------

fn show(env: &Env, id: &str) -> Result<Report, InstallerError> {
    let r = find(env, id)?;
    let view = travelling(env, &r);
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
            target_of(&env.root, &r)
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
        .data(json!({ "id": r.id, "state": r.state(), "target": target_of(&env.root, &r), "report": view })))
}

// ----- send -------------------------------------------------------------------------------

pub fn never_error(env: &Env, r: Option<&Saved>) -> InstallerError {
    if let Some(store) = env.cfg.locked_by("reports") {
        let kept = r.map_or_else(String::new, |r| {
            format!(
                "; the report stays at {}",
                crash::path_of(&env.root, r).display()
            )
        });
        return InstallerError::new(
            "E1857",
            format!(
                "reports is locked to \"never\" by {store}, so nothing leaves this machine{kept}"
            ),
        )
        .with_fix("ciginstaller config get reports")
        .with_exit(cigi_ui::EXIT_REFUSED);
    }
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

/// What `send` was asked to do about the route: all of them in order, or one.
pub struct Via {
    only: Option<routes::Route>,
    out: Option<PathBuf>,
}

impl Via {
    pub fn auto() -> Via {
        Via {
            only: None,
            out: None,
        }
    }
    fn parse(via: Option<&str>, out: Option<&Path>) -> Result<Via, InstallerError> {
        let only = match via {
            None => None,
            Some(w) => Some(routes::Route::parse(w).ok_or_else(|| {
                usage(format!(
                    "--via names a route: gh, api, link or file, not `{w}`"
                ))
            })?),
        };
        if only == Some(routes::Route::File) && out.is_none() {
            return Err(usage("--via file needs --out FILE"));
        }
        if only != Some(routes::Route::File) && out.is_some() {
            return Err(usage("--out goes with --via file"));
        }
        Ok(Via {
            only,
            out: out.map(Path::to_path_buf),
        })
    }
}

/// How a filing went, beyond the address.
#[derive(Default)]
struct Extra {
    how: Option<&'static str>,
    pointer: Option<String>,
}

/// Routes that could not be used, and the link to open by hand.
#[derive(Default)]
struct Left {
    unavailable: Vec<(String, String)>,
    link: Option<String>,
}

enum Sent {
    Filed(Option<String>, Extra),
    /// No route that files was usable; the report is kept.
    NoRoute(Left),
    AlreadyFiled(Option<String>),
    /// `--via link`: the link; the report stays queued.
    Link(String),
    /// `--via file`: written here; the report stays queued.
    Exported(PathBuf),
}

/// The repository a report is filed in: the one it was filed in, else the blamed product's.
pub fn target_of(root: &Path, r: &Saved) -> String {
    r.repo
        .clone()
        .filter(|x| !x.is_empty())
        .unwrap_or_else(|| blame::of(&route::travel(r, root)).repo)
}

fn mark_filed(env: &Env, r: &mut Saved, url: &str, repo: &str, number: u64) {
    r.sent = Some(cigi_home::now_rfc3339());
    r.issue = Some(crash::Issue {
        repo: repo.to_string(),
        number,
        url: url.to_string(),
    });
    r.queue.last_error = None;
    r.queue.next_try = None;
    r.queue.attempts += 1;
    let _ = crash::write(&env.root, r);
}

fn send_one(env: &Env, r: &mut Saved, via: &Via) -> Result<Sent, InstallerError> {
    if r.filed() {
        return Ok(Sent::AlreadyFiled(r.issue.as_ref().map(|i| i.url.clone())));
    }
    if crash::explicit_never(&env.cfg) {
        return Err(never_error(env, Some(r)));
    }
    let body = travelling(env, r);
    match via.only {
        Some(routes::Route::Link) => {
            let repo = target_of(&env.root, r);
            return routes::prefilled_link(&repo, &body)
                .map(|u| Sent::Link(u.to_string()))
                .ok_or_else(|| usage("the report is too long for a link; use `--via file`"));
        }
        Some(routes::Route::File) => {
            let path = via.out.clone().unwrap_or_default();
            routes::export(&path, &body).map_err(|e| {
                InstallerError::new("E1857", format!("could not export {}: {e}", r.id))
            })?;
            health::record(&env.root, "file", Ok(()));
            return Ok(Sent::Exported(path));
        }
        _ => {}
    }
    // a route the person runs themselves (round 1) is used alone when named and no route was asked
    let own = route::route_from_env().map_err(|e| {
        InstallerError::new("E1857", format!("the issues route cannot be used: {e}"))
            .with_fix("ciginstaller crash show")
    })?;
    r.queue.requested = true;
    if let (Some(route), None) = (own, via.only) {
        return send_own(env, r, &route, &body);
    }
    let mut left = Left::default();
    let mut failed: Vec<(&'static str, routes::Fail)> = Vec::new();
    for rt in [routes::Route::Gh, routes::Route::Api] {
        if via.only.is_some_and(|o| o != rt) {
            continue;
        }
        let client = match routes::client_for(rt) {
            Ok(c) => c,
            Err(why) => {
                left.unavailable.push((rt.word().to_string(), why));
                continue;
            }
        };
        match routes::file(&client, &body, r.repo.as_deref()) {
            Ok(f) => {
                health::record(&env.root, rt.word(), Ok(()));
                mark_filed(env, r, &f.url, &f.repo, f.number);
                let how = match f.how {
                    routes::How::Created => "created",
                    routes::How::Joined => "joined an issue that was already there",
                    routes::How::AlreadyThere => "already filed by this report",
                };
                return Ok(Sent::Filed(
                    Some(f.url),
                    Extra {
                        how: Some(how),
                        pointer: f.pointer,
                    },
                ));
            }
            Err(f) => {
                health::record(&env.root, rt.word(), Err(&f.why));
                failed.push((rt.word(), f));
            }
        }
    }
    if failed.is_empty() {
        let repo = target_of(&env.root, r);
        left.link = routes::prefilled_link(&repo, &body).map(|u| u.to_string());
        let _ = crash::write(&env.root, r);
        return Ok(Sent::NoRoute(left));
    }
    let why = failed
        .iter()
        .map(|(w, f)| format!("{w}: {}", f.why))
        .chain(
            left.unavailable
                .iter()
                .map(|(w, why)| format!("{w}: {why}")),
        )
        .collect::<Vec<_>>()
        .join("; ");
    r.queue.attempts += 1;
    r.queue.last_error = Some(why.clone());
    r.queue.next_try = failed
        .iter()
        .filter_map(|(_, f)| f.retry_at)
        .min()
        .map(health::rfc3339_of);
    let _ = crash::write(&env.root, r);
    Err(InstallerError::new(
        "E1857",
        format!(
            "could not send {}: {why}; it stays queued at {}",
            r.id,
            crash::path_of(&env.root, r).display()
        ),
    )
    .with_fix(format!(
        "ciginstaller crash test-route, then ciginstaller crash send {}",
        r.id
    ))
    .with_hint(format!(
        "or open the prefilled issue yourself: ciginstaller crash send {} --via link",
        r.id
    ))
    .with_subject(r.id.clone()))
}

/// `CIGI_REPORT_ROUTE`: one POST of what `show` prints, to the route the person runs.
fn send_own(
    env: &Env,
    r: &mut Saved,
    route: &url::Url,
    body: &Value,
) -> Result<Sent, InstallerError> {
    match route::post(route, body) {
        Ok(url) => {
            let issue = url.as_deref().and_then(routes::issue_of);
            r.sent = Some(cigi_home::now_rfc3339());
            r.issue = url
                .as_deref()
                .zip(issue)
                .map(|(u, (repo, number))| crash::Issue {
                    repo,
                    number,
                    url: u.to_string(),
                });
            r.queue.last_error = None;
            r.queue.attempts += 1;
            let _ = crash::write(&env.root, r);
            Ok(Sent::Filed(url, Extra::default()))
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
                    host_of(route),
                    f.why,
                    crash::path_of(&env.root, r).display()
                ),
            )
            .with_fix(format!("ciginstaller crash send {}", r.id))
            .with_subject(r.id.clone()))
        }
    }
}

fn no_route_report(env: &Env, ids: &[String], left: &Left) -> Report {
    let kept = ids.len();
    let mut rep = Report::new(
        Icon::Note,
        if kept == 1 {
            format!(
                "nothing was sent: there is no issues route yet; {} stays queued here",
                ids[0]
            )
        } else {
            format!(
                "nothing was sent: there is no issues route yet; {kept} reports stay queued here"
            )
        },
    );
    for (w, why) in &left.unavailable {
        rep = rep.detail(format!("route {w}: {why}"));
    }
    rep = rep
    .detail("the relay and the scoped token that file a report without a person's own login are still to come (#28), and the courier that sends a queued report even after a crash is #24")
    .detail(format!("the report(s) wait in {}; `ciginstaller crash show <report>` shows what would be sent", crash::ashtray_dir(&env.root).display()))
    .detail(format!("a route you run yourself: set {} to its https:// address; or log in with `gh auth login`, or set {} for the API", route::ROUTE_ENV, routes::TOKEN_ENV));
    if let (1, Some(l)) = (kept, &left.link) {
        rep = rep.detail(format!(
            "or open this prefilled issue yourself (nothing is requested by opening it here): {l}"
        ));
    }
    rep.data(json!({
        "sent": [], "queued": ids, "route": { "configured": false },
        "tried": left.unavailable.iter().map(|(w, why)| json!({ "route": w, "why": why })).collect::<Vec<_>>(),
        "link": if kept == 1 { json!(left.link) } else { Value::Null },
    }))
}

pub fn send(env: &Env, id: Option<&str>, all: bool) -> Result<Report, InstallerError> {
    send_via(env, id, all, &Via::auto())
}

fn send_via(env: &Env, id: Option<&str>, all: bool, via: &Via) -> Result<Report, InstallerError> {
    if all && via.only == Some(routes::Route::File) {
        return Err(usage("--via file exports one report: give its id"));
    }
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
    let mut handed: Vec<(String, String)> = Vec::new();
    let mut left = Left::default();
    for r in &mut targets {
        match send_one(env, r, via) {
            Ok(Sent::Filed(url, x)) => {
                sent.push(json!({ "id": r.id, "url": url, "how": x.how, "pointer": x.pointer }))
            }
            Ok(Sent::AlreadyFiled(url)) => known.push(json!({ "id": r.id, "url": url })),
            Ok(Sent::NoRoute(l)) => {
                queued.push(r.id.clone());
                left = l;
            }
            Ok(Sent::Link(u)) => handed.push((r.id.clone(), u)),
            Ok(Sent::Exported(p)) => handed.push((r.id.clone(), p.display().to_string())),
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
    if !handed.is_empty() {
        let file = via.only == Some(routes::Route::File);
        let mut rep = Report::done(if file {
            format!(
                "exported {}: it is not filed until it is imported or filed (#24 imports)",
                handed[0].0
            )
        } else {
            format!("{} prefilled link(s); nothing was requested and nothing is filed until you open one", handed.len())
        });
        for (i, w) in &handed {
            rep = rep.detail(format!("{i}: {w}"));
        }
        return Ok(rep.data(json!({
            "sent": [], "queued": handed.iter().map(|(i, _)| i).collect::<Vec<_>>(),
            "via": if file { "file" } else { "link" },
            "handed": handed.iter().map(|(i, w)| json!({ "id": i, "where": w })).collect::<Vec<_>>(),
        })));
    }
    if !queued.is_empty() && sent.is_empty() {
        return Ok(no_route_report(env, &queued, &left));
    }
    let mut rep = Report::done(match (sent.len(), known.len()) {
        (1, 0) => format!("sent {}", sent[0]["id"].as_str().unwrap_or("")),
        (n, 0) => format!("sent {n} reports"),
        (_, k) => format!("sent {}; {k} already filed", sent.len()),
    });
    for s in sent.iter().chain(known.iter()) {
        if let Some(u) = s["url"].as_str() {
            let how = s["how"]
                .as_str()
                .map(|h| format!(" ({h})"))
                .unwrap_or_default();
            rep = rep.detail(format!("{} is at {u}{how}", s["id"].as_str().unwrap_or("")));
        }
        if let Some(p) = s["pointer"].as_str() {
            rep = rep.detail(format!("a line was left in the issue it was seen in: {p}"));
        }
    }
    Ok(rep.data(json!({ "sent": sent, "already_filed": known, "queued": queued })))
}

// ----- test-route -------------------------------------------------------------------------

fn test_route(env: &Env, repo: Option<&str>) -> Result<Report, InstallerError> {
    let repo = repo.unwrap_or("otmof-ops/cig-installer");
    if repo.split('/').count() != 2
        || repo.split('/').any(str::is_empty)
        || repo.contains(char::is_whitespace)
    {
        return Err(usage(format!(
            "--repo must look like owner/name, not `{repo}`"
        )));
    }
    let skip = crash::explicit_never(&env.cfg);
    let mut probes = routes::blocked_probes();
    match route::route_from_env() {
        Ok(Some(u)) => probes.push(routes::Probe {
            route: "own-route",
            state: "note",
            detail: format!(
                "{} is set ({}): a POST route has no dry request, so it is not probed",
                route::ROUTE_ENV,
                host_of(&u)
            ),
            retry_at: None,
        }),
        Ok(None) => {}
        Err(e) => probes.push(routes::Probe {
            route: "own-route",
            state: "failed",
            detail: e,
            retry_at: None,
        }),
    }
    for rt in routes::ORDER {
        let p = routes::probe(rt, repo, skip);
        match p.state {
            "ok" if rt.files() => health::record(&env.root, p.route, Ok(())),
            "failed" => health::record(&env.root, p.route, Err(&p.detail)),
            _ => {}
        }
        probes.push(p);
    }
    let filing: Vec<&str> = probes
        .iter()
        .filter(|p| p.state == "ok" && matches!(p.route, "gh" | "api"))
        .map(|p| p.route)
        .collect();
    let own = probes
        .iter()
        .any(|p| p.route == "own-route" && p.state == "note");
    let data = json!({
        "repo": repo,
        "routes": probes.iter().map(|p| json!({
            "route": p.route, "state": p.state, "detail": p.detail,
            "retry_at": p.retry_at.map(health::rfc3339_of),
        })).collect::<Vec<_>>(),
        "can_file": filing,
        "reports_setting": consent_word(crash::consent(&env.cfg).0),
    });
    let line = |p: &routes::Probe| format!("{:<13}{:<12}{}", p.route, p.state, p.detail);
    if filing.is_empty() && !own && !skip {
        let table = probes.iter().map(line).collect::<Vec<_>>().join("\n");
        return Err(InstallerError::new(
            "E1857",
            format!(
                "no route can file a report right now: {}",
                probes
                    .iter()
                    .filter(|p| matches!(p.route, "gh" | "api"))
                    .map(|p| format!("{}: {}", p.route, p.detail))
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
        )
        .with_hint(table)
        .with_fix("gh auth login, or set CIG_GITHUB_TOKEN; `ciginstaller crash send <id> --via link` always works"));
    }
    let mut rep = Report::done(if skip {
        "reports is \"never\": no route was probed".to_string()
    } else if filing.is_empty() {
        "your own route is set; no other route can file".to_string()
    } else {
        format!("a report can leave by: {}", filing.join(", "))
    });
    for p in &probes {
        rep = rep.detail(line(p));
    }
    Ok(rep.data(data))
}

// ----- status -----------------------------------------------------------------------------

fn status(env: &Env) -> Result<Report, InstallerError> {
    let l =
        crash::list(&env.root).map_err(|e| usage(format!("the ashtray cannot be read: {e}")))?;
    let (consent, _) = crash::consent(&env.cfg);
    let queue: Vec<&Saved> = l.reports.iter().filter(|r| !r.filed()).collect();
    let log = health::load(&env.root);
    let data = json!({
        "queue": queue.iter().map(|r| json!({
            "id": r.id, "origin": r.origin, "kind": r.kind.word(),
            "target": target_of(&env.root, r), "state": r.state(), "requested": r.queue.requested,
            "attempts": r.queue.attempts, "last_error": r.queue.last_error, "next_try": r.queue.next_try,
        })).collect::<Vec<_>>(),
        "queued": queue.len(),
        "filed": l.reports.len() - queue.len(),
        "reports_setting": consent_word(consent),
        "routes": log.iter().map(|(k, v)| json!({
            "route": k, "last_success": v.last_success, "last_failure": v.last_failure, "last_error": v.last_error,
        })).collect::<Vec<_>>(),
        "blocked": ["relay", "scoped-token", "courier flush after a crash (#24)"],
    });
    let mut rep = Report::done(format!(
        "{} report(s) queued, {} filed; reports = {}",
        queue.len(),
        l.reports.len() - queue.len(),
        consent_word(consent)
    ));
    for r in &queue {
        rep = rep.detail(format!(
            "{}  {}  {}  to {}  attempts {}{}{}",
            r.id,
            r.origin,
            r.kind.word(),
            target_of(&env.root, r),
            r.queue.attempts,
            r.queue
                .last_error
                .as_ref()
                .map(|e| format!("  last error: {}", crash::truncate(e, 120)))
                .unwrap_or_default(),
            r.queue
                .next_try
                .as_ref()
                .map(|t| format!("  next try after {t}"))
                .unwrap_or_default(),
        ));
    }
    for (k, v) in &log {
        rep = rep.detail(format!(
            "route {k}: last success {}, last failure {}{}",
            v.last_success.as_deref().unwrap_or("never"),
            v.last_failure.as_deref().unwrap_or("never"),
            v.last_error
                .as_ref()
                .map(|e| format!(" ({})", crash::truncate(e, 100)))
                .unwrap_or_default()
        ));
    }
    rep = rep.detail("the relay, the scoped token and the courier that sends a queued report after a crash are #24, blocked on otmof-ops/CigScript#105");
    Ok(rep.next("ciginstaller crash test-route").data(data))
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
        return match send_one(env, &mut r, &Via::auto()) {
            Ok(Sent::Filed(url, _)) => Ok(Report::done(format!(
                "saved as {id} and sent{}",
                url.map(|u| format!(": {u}")).unwrap_or_default()
            ))
            .data(json!({ "id": id, "sent": true }))),
            Ok(Sent::NoRoute(l)) => Ok(no_route_report(env, std::slice::from_ref(&id), &l)),
            Ok(Sent::Link(_) | Sent::Exported(_)) => {
                Ok(rep.data(json!({ "id": id, "sent": false })))
            }
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
