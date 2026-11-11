// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! The routes a report can leave by (#28 section 4), tried in order, each with its dry check.
//!
//! 1. the courier's relay and 2. a scoped token: #24, blocked on otmof-ops/CigScript#105;
//! 3. `gh`, with the person's own login (found on PATH);
//! 4. the API, with `CIG_GITHUB_TOKEN`, through the installer's own HTTP client, reading
//!    GitHub's error body and showing it (cig-script#126: never hidden behind a bare status);
//! 5. a prefilled issue link: always available, never a request;
//! 6. export to a file.
//!
//! A route files a report the same way whichever it is: search for the fingerprint in the blamed
//! product's repository (never create blind), join the issue that exists (following a moved
//! issue), or create it; then leave a line in the origin's own issue when the fault was blamed
//! on another product. A report this one already made is recognised by its id, so a lost
//! response never files it twice. All text comes from `route::travel`'s object.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::{json, Value};
use url::Url;

use super::blame::{self, Blame};
use super::{block, health, truncate};

/// How long `gh api` may take before it is stopped.
const GH_CALL_SECS: u64 = 45;
/// How long `gh auth status` (local: it reads gh's own login) may take.
const GH_AUTH_SECS: u64 = 10;

pub const API_ENV: &str = "CIGI_GITHUB_API";
pub const TOKEN_ENV: &str = "CIG_GITHUB_TOKEN";
/// Only ever narrows: `gh,api,link,file`, or `none`. Not authentication of any kind.
pub const ROUTES_ENV: &str = "CIGI_ROUTES";
pub const DEFAULT_API: &str = "https://api.github.com";
const BLOCKED: &str = "blocked on #24 (the courier), which waits on otmof-ops/CigScript#105";
const MAX_HOPS: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    Gh,
    Api,
    Link,
    File,
}

pub const ORDER: [Route; 4] = [Route::Gh, Route::Api, Route::Link, Route::File];

impl Route {
    pub fn word(self) -> &'static str {
        match self {
            Route::Gh => "gh",
            Route::Api => "api",
            Route::Link => "link",
            Route::File => "file",
        }
    }
    pub fn parse(s: &str) -> Option<Route> {
        ORDER.into_iter().find(|r| r.word() == s.trim())
    }
    /// Does it file the report itself (as against giving the person a way to)?
    pub fn files(self) -> bool {
        matches!(self, Route::Gh | Route::Api)
    }
}

/// May this route run? `CIGI_ROUTES` can only take routes away.
pub fn enabled(r: Route) -> bool {
    match std::env::var(ROUTES_ENV) {
        Err(_) => true,
        Ok(v) => {
            let v = v.trim();
            v != "none" && v.split(',').any(|w| w.trim() == r.word())
        }
    }
}

pub fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Why a call did not work, in words a person can act on, with GitHub's own message in them.
#[derive(Clone, Debug)]
pub struct Fail {
    pub why: String,
    /// Epoch second a rate limit lifts, when it said.
    pub retry_at: Option<i64>,
}

impl Fail {
    fn plain(why: impl Into<String>) -> Fail {
        Fail {
            why: why.into(),
            retry_at: None,
        }
    }
}

/// One issue, as a search or a create answered.
#[derive(Clone, Debug)]
pub struct Hit {
    pub repo: String,
    pub number: u64,
    pub url: String,
    pub body: String,
}

/// `https://host/owner/name/issues/N` as its repository and number.
pub fn issue_of(url: &str) -> Option<(String, u64)> {
    let u = Url::parse(url).ok()?;
    let parts: Vec<&str> = u.path_segments()?.collect();
    let i = parts.iter().position(|p| *p == "issues")?;
    let number = parts.get(i + 1)?.parse().ok()?;
    (i >= 2).then(|| (format!("{}/{}", parts[i - 2], parts[i - 1]), number))
}

fn hit_of(v: &Value, fallback_repo: &str) -> Option<Hit> {
    let url = v["html_url"].as_str()?.to_string();
    let (repo, number) = issue_of(&url).unwrap_or_else(|| {
        (
            fallback_repo.to_string(),
            v["number"].as_u64().unwrap_or_default(),
        )
    });
    Some(Hit {
        repo,
        number,
        url,
        body: v["body"].as_str().unwrap_or("").to_string(),
    })
}

// ----- the clients ------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub enum Client {
    Gh(PathBuf),
    Api { base: Url, token: String },
}

/// The API base: `CIGI_GITHUB_API`, https or loopback http, no query, fragment or credentials.
pub fn api_base() -> Result<Url, String> {
    let raw = std::env::var(API_ENV).unwrap_or_else(|_| DEFAULT_API.to_string());
    let u = Url::parse(raw.trim()).map_err(|e| format!("{API_ENV} is not a URL: {e}"))?;
    cigi_source::http::check_scheme(&u).map_err(|e| format!("{API_ENV}: {e}"))?;
    if u.query().is_some()
        || u.fragment().is_some()
        || !u.username().is_empty()
        || u.password().is_some()
    {
        return Err(format!(
            "{API_ENV} carries no query, fragment or credentials"
        ));
    }
    Ok(u)
}

pub fn find_gh() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for name in ["gh", "gh.exe"] {
            let p = dir.join(name);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// `spawn`, tried again (a few times, a few milliseconds apart) while the program file is still
/// being closed by whoever wrote it: Linux answers "text file busy" for a moment.
fn spawn_retry(cmd: &mut Command) -> std::io::Result<std::process::Child> {
    let mut tries = 0;
    loop {
        match cmd.spawn() {
            Err(e) if e.raw_os_error() == Some(26) && tries < 5 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(10));
            }
            r => return r,
        }
    }
}

/// Run `cmd` with `input` on its stdin and wait at most `secs`. A child still running at the
/// deadline is killed and reaped, and the answer is `Ok(None)`: a hung `gh` (a keyring prompt, a
/// stalled network) never holds the installer or outlives it.
fn run_timed(
    cmd: &mut Command,
    input: Option<Vec<u8>>,
    secs: u64,
) -> std::io::Result<Option<std::process::Output>> {
    let mut child = spawn_retry(cmd)?;
    if let Some(mut si) = child.stdin.take() {
        // written from its own thread, so a child that never reads cannot block the deadline
        std::thread::spawn(move || {
            if let Some(b) = input {
                let _ = si.write_all(&b);
            }
        });
    }
    let drain = |r: Option<Box<dyn Read + Send>>| {
        r.map(|mut r| {
            std::thread::spawn(move || {
                let mut v = Vec::new();
                let _ = r.read_to_end(&mut v);
                v
            })
        })
    };
    let out = drain(
        child
            .stdout
            .take()
            .map(|r| Box::new(r) as Box<dyn Read + Send>),
    );
    let err = drain(
        child
            .stderr
            .take()
            .map(|r| Box::new(r) as Box<dyn Read + Send>),
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    let status = loop {
        if let Some(st) = child.try_wait()? {
            break st;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            // the readers are left to finish on their own: a grandchild may still hold a pipe
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let join = |h: Option<std::thread::JoinHandle<Vec<u8>>>| {
        h.and_then(|h| h.join().ok()).unwrap_or_default()
    };
    Ok(Some(std::process::Output {
        status,
        stdout: join(out),
        stderr: join(err),
    }))
}

/// The client for a filing route, or why it cannot be used. Nothing is requested here, except
/// that `gh auth status` is asked (it is local: it reads gh's own login).
pub fn client_for(route: Route) -> Result<Client, String> {
    if !enabled(route) {
        return Err(format!("turned off by {ROUTES_ENV}"));
    }
    match route {
        Route::Gh => {
            let gh = find_gh().ok_or("gh was not found on PATH")?;
            let ok = run_timed(
                Command::new(&gh)
                    .args(["auth", "status", "--hostname", "github.com"])
                    .env("GH_PROMPT_DISABLED", "1")
                    .env("NO_COLOR", "1")
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null()),
                None,
                GH_AUTH_SECS,
            )
            .ok()
            .flatten()
            .is_some_and(|o| o.status.success());
            if ok {
                Ok(Client::Gh(gh))
            } else {
                Err("gh is not logged in (run `gh auth login`)".into())
            }
        }
        Route::Api => {
            let token = std::env::var(TOKEN_ENV).unwrap_or_default();
            let token = token.trim().to_string();
            if token.is_empty() {
                return Err(format!("{TOKEN_ENV} is not set"));
            }
            if token.chars().any(|c| c.is_whitespace() || c.is_control()) {
                return Err(format!("{TOKEN_ENV} is not a usable token"));
            }
            Ok(Client::Api {
                base: api_base()?,
                token,
            })
        }
        _ => Err("this route does not file".into()),
    }
}

fn scrub(text: &str, token: Option<&str>) -> String {
    match token {
        Some(t) if !t.is_empty() => text.replace(t, "<token>"),
        _ => text.to_string(),
    }
}

/// What GitHub said, shown whole enough to act on: its message and its errors.
fn github_says(status: u16, text: &str, token: Option<&str>) -> String {
    let shown = match serde_json::from_str::<Value>(text) {
        Ok(v) if v["message"].is_string() => {
            let mut s = v["message"].as_str().unwrap_or("").to_string();
            if let Some(e) = v.get("errors").filter(|e| !e.is_null()) {
                s.push_str(&format!(" {e}"));
            }
            s
        }
        _ => text.trim().to_string(),
    };
    scrub(
        &format!("GitHub answered {status}: {}", truncate(&shown, 400)),
        token,
    )
}

fn classify(
    status: u16,
    text: &str,
    header: &dyn Fn(&str) -> Option<String>,
    token: Option<&str>,
) -> Fail {
    let remaining = header("x-ratelimit-remaining");
    let lower = text.to_ascii_lowercase();
    let limited = status == 429
        || (status == 403
            && (remaining.as_deref() == Some("0")
                || lower.contains("rate limit")
                || lower.contains("abuse")));
    let mut why = github_says(status, text, token);
    let mut retry_at = None;
    if limited {
        retry_at = header("retry-after")
            .and_then(|s| s.parse::<i64>().ok())
            .map(|s| now_epoch() + s)
            .or_else(|| header("x-ratelimit-reset").and_then(|s| s.parse().ok()));
        let until = retry_at.map_or("later".to_string(), health::rfc3339_of);
        why = format!("rate limited until {until}: {why}");
    } else if status == 401 {
        why = format!("the token was refused: {why}");
    } else if status == 403 || status == 404 {
        why = format!("the login or token cannot do this (does it have the issues scope?): {why}");
    }
    Fail { why, retry_at }
}

impl Client {
    fn call(
        &self,
        method: &str,
        path: &str,
        query: &[(&str, String)],
        body: Option<&Value>,
    ) -> Result<Value, Fail> {
        match self {
            Client::Api { base, token } => api_call(base, token, method, path, query, body),
            Client::Gh(gh) => gh_call(gh, method, path, query, body),
        }
    }

    pub fn search(&self, q: &str) -> Result<Vec<Hit>, Fail> {
        let v = self.call(
            "GET",
            "search/issues",
            &[("q", q.to_string()), ("per_page", "5".into())],
            None,
        )?;
        Ok(v["items"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|i| hit_of(i, ""))
            .collect())
    }

    /// The issue at an address, following a transfer to where it lives now.
    pub fn get_issue(&self, repo: &str, number: u64) -> Result<Hit, Fail> {
        let v = self.call("GET", &format!("repos/{repo}/issues/{number}"), &[], None)?;
        hit_of(&v, repo).ok_or_else(|| Fail::plain("GitHub's answer for the issue had no address"))
    }

    pub fn create(&self, repo: &str, r: &block::Rendered) -> Result<Hit, Fail> {
        let body = json!({ "title": r.title, "body": r.body, "labels": r.labels });
        let v = self.call("POST", &format!("repos/{repo}/issues"), &[], Some(&body))?;
        hit_of(&v, repo)
            .ok_or_else(|| Fail::plain("GitHub's answer for the new issue had no address"))
    }

    pub fn comment(&self, repo: &str, number: u64, text: &str) -> Result<(), Fail> {
        self.call(
            "POST",
            &format!("repos/{repo}/issues/{number}/comments"),
            &[],
            Some(&json!({ "body": text })),
        )
        .map(|_| ())
    }
}

fn api_call(
    base: &Url,
    token: &str,
    method: &str,
    path: &str,
    query: &[(&str, String)],
    body: Option<&Value>,
) -> Result<Value, Fail> {
    let mut url = base.clone();
    url.set_path(&format!(
        "{}/{}",
        base.path().trim_end_matches('/'),
        path.trim_start_matches('/')
    ));
    if !query.is_empty() {
        let mut qp = url.query_pairs_mut();
        for (k, v) in query {
            qp.append_pair(k, v);
        }
    }
    let agent = ureq::AgentBuilder::new()
        .tls_config(cigi_source::http::tls_config())
        .user_agent(cigi_source::http::USER_AGENT)
        .redirects(0)
        .timeout_connect(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build();
    let tok = Some(token);
    for _ in 0..=MAX_HOPS {
        let mut req = agent
            .request(method, url.as_str())
            .set("Accept", "application/vnd.github+json")
            .set("X-GitHub-Api-Version", "2022-11-28")
            .set("Authorization", &format!("Bearer {token}"));
        let sent = match body {
            Some(b) => req
                .set("Content-Type", "application/json")
                .send_string(&b.to_string()),
            None => {
                req = req.set("Accept", "application/vnd.github+json");
                req.call()
            }
        };
        let resp = match sent {
            Ok(r) => r,
            Err(ureq::Error::Status(code, r)) => {
                let h = |k: &str| r.header(k).map(str::to_string);
                let (rl, ra, rs) = (
                    h("x-ratelimit-remaining"),
                    h("retry-after"),
                    h("x-ratelimit-reset"),
                );
                let mut text = String::new();
                let _ = r.into_reader().take(65_536).read_to_string(&mut text);
                let lookup = move |k: &str| match k {
                    "x-ratelimit-remaining" => rl.clone(),
                    "retry-after" => ra.clone(),
                    "x-ratelimit-reset" => rs.clone(),
                    _ => None,
                };
                return Err(classify(code, &text, &lookup, tok));
            }
            Err(ureq::Error::Transport(t)) => {
                let host = url.host_str().unwrap_or("?");
                let what = match t.kind() {
                    ureq::ErrorKind::Dns => format!("the name {host} could not be looked up (DNS)"),
                    ureq::ErrorKind::ConnectionFailed => {
                        format!("could not connect to {host} (network down?)")
                    }
                    k => format!("could not reach {host} ({k})"),
                };
                return Err(Fail::plain(what));
            }
        };
        let status = resp.status();
        if (300..400).contains(&status) {
            // a moved issue: follow it, to the same origin only (the token goes nowhere else)
            let Some(to) = resp.header("location").and_then(|l| url.join(l).ok()) else {
                return Err(Fail::plain(format!(
                    "GitHub answered {status} with no place to go"
                )));
            };
            if method != "GET" || to.origin() != base.origin() {
                return Err(Fail::plain(format!(
                    "GitHub answered {status}, a redirect that is not followed (a report goes only where it was told to)"
                )));
            }
            url = to;
            continue;
        }
        let mut text = String::new();
        let _ = resp.into_reader().take(1 << 20).read_to_string(&mut text);
        return serde_json::from_str(&text)
            .map_err(|_| Fail::plain(format!("GitHub's answer ({status}) was not JSON")));
    }
    Err(Fail::plain("too many redirects"))
}

fn gh_call(
    gh: &Path,
    method: &str,
    path: &str,
    query: &[(&str, String)],
    body: Option<&Value>,
) -> Result<Value, Fail> {
    let mut cmd = Command::new(gh);
    cmd.args([
        "api",
        "-X",
        method,
        path,
        "-H",
        "Accept: application/vnd.github+json",
    ]);
    for (k, v) in query {
        cmd.arg("-f").arg(format!("{k}={v}"));
    }
    if body.is_some() {
        cmd.args(["--input", "-"]);
    }
    cmd.env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env("NO_COLOR", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let input = body.map(|b| b.to_string().into_bytes());
    let out = match run_timed(&mut cmd, input, GH_CALL_SECS) {
        Ok(Some(o)) => o,
        Ok(None) => {
            return Err(Fail::plain(format!(
                "gh did not answer in {GH_CALL_SECS} seconds and was stopped"
            )))
        }
        Err(e) => return Err(Fail::plain(format!("gh could not be started: {e}"))),
    };
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if out.status.success() {
        return serde_json::from_str(&stdout).map_err(|_| Fail::plain("gh's answer was not JSON"));
    }
    let status = stderr
        .split("HTTP ")
        .nth(1)
        .and_then(|s| s.get(..3))
        .and_then(|s| s.parse::<u16>().ok());
    let said = if stdout.trim().is_empty() {
        stderr.trim().to_string()
    } else {
        stdout.trim().to_string()
    };
    let none = |_: &str| None;
    Err(match status {
        Some(code) => classify(code, &said, &none, None),
        None => Fail {
            why: format!("gh failed: {}", truncate(&scrub(&said, None), 400)),
            retry_at: None,
        },
    })
}

// ----- filing ----------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum How {
    /// A new issue.
    Created,
    /// One more sighting on the issue that was there.
    Joined,
    /// This very report was already filed (a response was lost): nothing more was written.
    AlreadyThere,
}

#[derive(Clone, Debug)]
pub struct Filed {
    pub url: String,
    pub repo: String,
    pub number: u64,
    pub how: How,
    /// A line was left in the origin's issue, at this address.
    pub pointer: Option<String>,
}

/// File the report through one client. `view` is `route::travel`'s object.
pub fn file(c: &Client, view: &Value, repo_override: Option<&str>) -> Result<Filed, Fail> {
    let mut blamed = blame::of(view);
    if let Some(r) = repo_override.filter(|r| !r.is_empty()) {
        blamed.repo = r.to_string();
    }
    let id = view["id"].as_str().unwrap_or("-");
    let rendered = block::render(view, &blamed);
    let found = c.search(&block::search_query(view, &blamed.repo))?;
    if let Some(mine) = found
        .iter()
        .find(|h| block::parse(&h.body).is_ok_and(|b| b["id"] == id))
    {
        let now = c.get_issue(&mine.repo, mine.number)?;
        return Ok(Filed {
            url: now.url,
            repo: now.repo,
            number: now.number,
            how: How::AlreadyThere,
            pointer: None,
        });
    }
    let (issue, how) = match found.first() {
        Some(h) => {
            // follow a transfer: the issue as it lives now
            let now = c.get_issue(&h.repo, h.number)?;
            c.comment(&now.repo, now.number, &block::seen_again(view, &blamed))?;
            (now, How::Joined)
        }
        None => (c.create(&blamed.repo, &rendered)?, How::Created),
    };
    let pointer = leave_pointer(c, view, &blamed, &issue.url);
    Ok(Filed {
        url: issue.url,
        repo: issue.repo,
        number: issue.number,
        how,
        pointer,
    })
}

/// The origin's own issue, when the fault is another product's: a line saying where it went.
/// Best effort: the report is filed whether or not this works.
fn leave_pointer(c: &Client, view: &Value, blamed: &Blame, url: &str) -> Option<String> {
    let origin = blame::origin_repo(view);
    if origin == blamed.repo {
        return None;
    }
    let hit = c
        .search(&block::search_query(view, &origin))
        .ok()?
        .into_iter()
        .next()?;
    let now = c.get_issue(&hit.repo, hit.number).ok()?;
    c.comment(
        &now.repo,
        now.number,
        &block::pointer_line(view, blamed, url),
    )
    .ok()?;
    Some(now.url)
}

// ----- the link and the file --------------------------------------------------------------

/// GitHub's prefilled new-issue page. Built, never requested; kept under a length browsers and
/// GitHub accept, with the block when it fits and the readable part alone when it does not.
pub fn prefilled_link(repo: &str, view: &Value) -> Option<Url> {
    let blamed = blame::of(view);
    let bl = block::machine_block(view, &blamed);
    let mut compact = bl.clone();
    if let Some(o) = compact.as_object_mut() {
        o.insert("repro".into(), Value::Null);
    }
    let human = block::human_short(view, &blamed);
    for body in [format!("{human}\n{}", block::fenced(&compact)), human] {
        let mut u = Url::parse(&format!("https://github.com/{repo}/issues/new")).ok()?;
        u.query_pairs_mut()
            .append_pair("title", &block::title(view))
            .append_pair("body", &body)
            .append_pair("labels", &block::labels(view, &blamed).join(","));
        if u.as_str().len() <= 7_000 {
            return Some(u);
        }
    }
    None
}

/// Export for another machine (import is #24): the issue as it would be filed, in a file only
/// the owner can read. An existing file is never overwritten.
pub fn export(path: &Path, view: &Value) -> Result<(), String> {
    let blamed = blame::of(view);
    let r = block::render(view, &blamed);
    let doc = json!({
        "cig-report-export": 1,
        "repo": blamed.repo,
        "title": r.title, "labels": r.labels, "body": r.body,
        "report": view,
    });
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    let mut f = o
        .open(path)
        .map_err(|e| format!("{} could not be created: {e}", path.display()))?;
    f.write_all(
        serde_json::to_string_pretty(&doc)
            .map_err(|e| e.to_string())?
            .as_bytes(),
    )
    .map_err(|e| format!("{} could not be written: {e}", path.display()))
}

// ----- the dry check ---------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Probe {
    pub route: &'static str,
    /// `ok`, `failed`, `unavailable`, `blocked`, `skipped` or `note`.
    pub state: &'static str,
    pub detail: String,
    pub retry_at: Option<i64>,
}

fn probe_of(route: &'static str, state: &'static str, detail: impl Into<String>) -> Probe {
    Probe {
        route,
        state,
        detail: detail.into(),
        retry_at: None,
    }
}

/// The relay and the scoped token, which are the courier's.
pub fn blocked_probes() -> Vec<Probe> {
    vec![
        probe_of("relay", "blocked", BLOCKED),
        probe_of("scoped-token", "blocked", BLOCKED),
    ]
}

/// A route's dry request: a search, issues only, nothing written. With `skip` (reports is
/// `never`) nothing is asked of anyone.
pub fn probe(route: Route, repo: &str, skip: bool) -> Probe {
    let w = route.word();
    match route {
        Route::Link => probe_of(
            w,
            "ok",
            "always available; opening it is yours, no request is made",
        ),
        Route::File => probe_of(
            w,
            "ok",
            "always available: `crash send <id> --via file --out FILE`",
        ),
        Route::Gh | Route::Api => {
            if skip {
                return probe_of(w, "skipped", "not probed: reports is \"never\"");
            }
            let client = match client_for(route) {
                Ok(c) => c,
                Err(why) => return probe_of(w, "unavailable", why),
            };
            let q = format!("repo:{repo} is:issue in:title ciginstaller-route-test");
            match client.search(&q) {
                Ok(h) => probe_of(
                    w,
                    "ok",
                    format!("a dry search of {repo} answered ({} result(s))", h.len()),
                ),
                Err(f) => Probe {
                    route: w,
                    state: "failed",
                    detail: f.why,
                    retry_at: f.retry_at,
                },
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn a_child_that_outlives_its_deadline_is_stopped() {
        let start = std::time::Instant::now();
        let mut cmd = Command::new("sleep");
        cmd.arg("30")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let got = run_timed(&mut cmd, Some(b"never read".to_vec()), 1).unwrap();
        assert!(got.is_none(), "a hung child is reported as stopped");
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "{:?}",
            start.elapsed()
        );
    }

    #[test]
    fn a_child_that_answers_in_time_gives_its_output() {
        let mut cmd = Command::new("cat");
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let got = run_timed(&mut cmd, Some(b"{\"ok\":true}".to_vec()), 10)
            .unwrap()
            .expect("cat answers at once");
        assert!(got.status.success());
        assert_eq!(got.stdout, b"{\"ok\":true}");
    }
}
