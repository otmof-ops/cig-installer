// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! A fake issue tracker (#28 step 2): a loopback server that speaks the slice of GitHub's REST
//! API the report routes use, so every route is tested end to end without the network.
//!
//! - `GET  /search/issues?q=...`: 422 unless the query says `is:issue` and `repo:` (the
//!   cig-script#126 failure), else the stored issues that hold every plain word of the query
//!   (a transferred issue is still listed at its old address: the index lags).
//! - `POST /repos/{owner}/{repo}/issues`: 201 with the new issue.
//! - `GET  /repos/{owner}/{repo}/issues/{n}`: the issue, or a 301 to where it moved.
//! - `POST /repos/{owner}/{repo}/issues/{n}/comments`: 201.
//!
//! Faults are set per endpoint and count down, so "fails once, then works" is one call. The
//! server stops when dropped and on its own after [`LIFETIME`].

use std::collections::HashMap;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tiny_http::{Header, Response, Server, StatusCode};

use crate::must;

/// The hard stop: a test server never outlives this.
pub const LIFETIME: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Endpoint {
    Search,
    Create,
    Get,
    Comment,
    /// Every endpoint, checked after the specific ones.
    Any,
}

#[derive(Debug, Clone)]
pub enum Fault {
    /// This status with GitHub's error shape: `{"message": ..., "documentation_url": ...}`.
    Status(u16, String),
    /// 403 or 429 with `x-ratelimit-remaining: 0`, `x-ratelimit-reset` and `retry-after`.
    RateLimit(u16),
    /// The write is made, then the answer is this status: the response that got lost.
    AnswerAfterWrite(u16),
}

#[derive(Debug, Clone)]
pub struct TrackerRequest {
    pub method: String,
    pub path: String,
    pub query: Option<String>,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl TrackerRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
    /// The decoded `q` of a search.
    pub fn q(&self) -> Option<String> {
        let q = self.query.as_deref()?;
        let raw = q.split('&').find_map(|p| p.strip_prefix("q="))?;
        Some(percent_decode(raw))
    }
}

#[derive(Debug, Clone)]
pub struct TrackerIssue {
    pub repo: String,
    pub number: u64,
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
    pub comments: Vec<String>,
}

#[derive(Default)]
struct State {
    issues: Vec<TrackerIssue>,
    /// (repo, number) -> (new repo, new number)
    moved: HashMap<(String, u64), (String, u64)>,
    faults: HashMap<Endpoint, (Fault, u32)>,
    token: Option<String>,
    log: Vec<TrackerRequest>,
}

type Shared = Arc<Mutex<State>>;

pub struct FakeTracker {
    server: Arc<Server>,
    addr: std::net::SocketAddr,
    state: Shared,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl FakeTracker {
    pub fn start() -> FakeTracker {
        let server = Arc::new(must(
            Server::http("127.0.0.1:0").map_err(|e| e.to_string()),
            "bind loopback",
        ));
        let addr = must(
            server.server_addr().to_ip().ok_or("not an ip address"),
            "tracker address",
        );
        let state: Shared = Arc::default();
        let stop = Arc::new(AtomicBool::new(false));
        let base = format!("http://{addr}");
        let handle = {
            let (server, state, stop) = (server.clone(), state.clone(), stop.clone());
            std::thread::spawn(move || {
                let deadline = Instant::now() + LIFETIME;
                while !stop.load(Ordering::Relaxed) && Instant::now() < deadline {
                    if let Ok(Some(req)) = server.recv_timeout(Duration::from_millis(50)) {
                        handle(req, &state, &base);
                    }
                }
            })
        };
        FakeTracker {
            server,
            addr,
            state,
            stop,
            handle: Some(handle),
        }
    }

    /// `http://127.0.0.1:PORT`: the API base for a test (loopback http is allowed).
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Writes and searches need `Authorization: Bearer <token>`; without it, 401.
    pub fn require_token(&self, token: &str) {
        self.lock().token = Some(token.to_string());
    }

    /// Fail `endpoint` the next `times` requests.
    pub fn fail(&self, endpoint: Endpoint, fault: Fault, times: u32) {
        self.lock().faults.insert(endpoint, (fault, times));
    }

    pub fn clear_faults(&self) {
        self.lock().faults.clear();
    }

    pub fn seed_issue(&self, repo: &str, title: &str, body: &str) -> u64 {
        let mut s = self.lock();
        let n = next_number(&s, repo);
        s.issues.push(TrackerIssue {
            repo: repo.into(),
            number: n,
            title: title.into(),
            body: body.into(),
            labels: Vec::new(),
            comments: Vec::new(),
        });
        n
    }

    /// Move an issue to another repository, as a maintainer's transfer does: the old address
    /// answers 301 to the new one. Returns the new number.
    pub fn transfer(&self, repo: &str, number: u64, to_repo: &str) -> u64 {
        let mut s = self.lock();
        let n = next_number(&s, to_repo);
        if let Some(i) = s
            .issues
            .iter_mut()
            .find(|i| i.repo == repo && i.number == number)
        {
            i.repo = to_repo.into();
            i.number = n;
        }
        s.moved.insert((repo.into(), number), (to_repo.into(), n));
        n
    }

    pub fn issues(&self) -> Vec<TrackerIssue> {
        self.lock().issues.clone()
    }
    pub fn requests(&self) -> Vec<TrackerRequest> {
        self.lock().log.clone()
    }
    pub fn clear_requests(&self) {
        self.lock().log.clear();
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        lock(&self.state)
    }
}

impl Drop for FakeTracker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.server.unblock();
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn lock(s: &Shared) -> MutexGuard<'_, State> {
    match s.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

fn next_number(s: &State, repo: &str) -> u64 {
    s.issues
        .iter()
        .filter(|i| i.repo == repo)
        .map(|i| i.number)
        .chain(
            s.moved
                .iter()
                .filter(|((r, _), _)| r == repo)
                .map(|((_, n), _)| *n),
        )
        .max()
        .unwrap_or(0)
        + 1
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                let h = std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(h, 16) {
                    Ok(v) => {
                        out.push(v);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn header(k: &str, v: &str) -> Option<Header> {
    Header::from_bytes(k.as_bytes(), v.as_bytes()).ok()
}

fn json_reply(
    code: u16,
    body: &str,
    extra: &[(&str, String)],
) -> Response<std::io::Cursor<Vec<u8>>> {
    let mut r = Response::from_data(body.as_bytes().to_vec()).with_status_code(StatusCode(code));
    for (k, v) in extra {
        if let Some(h) = header(k, v) {
            r = r.with_header(h);
        }
    }
    if let Some(h) = header("Content-Type", "application/json; charset=utf-8") {
        r = r.with_header(h);
    }
    r
}

fn esc(s: &str) -> String {
    serde_str(s)
}

/// A JSON string literal (std only: the testkit has no serde).
fn serde_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn issue_json(i: &TrackerIssue, base: &str) -> String {
    format!(
        "{{\"number\":{},\"title\":{},\"body\":{},\"state\":\"open\",\"html_url\":{},\"url\":{},\"repository_url\":{}}}",
        i.number,
        esc(&i.title),
        esc(&i.body),
        esc(&format!("{base}/{}/issues/{}", i.repo, i.number)),
        esc(&format!("{base}/repos/{}/issues/{}", i.repo, i.number)),
        esc(&format!("{base}/repos/{}", i.repo)),
    )
}

/// The value of a string field in a flat-enough JSON object (the testkit has no JSON parser).
fn json_field(body: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\"");
    let at = body.find(&pat)? + pat.len();
    let rest = body[at..].trim_start().strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let mut out = String::new();
    let mut it = rest.chars();
    while let Some(c) = it.next() {
        match c {
            '"' => return Some(out),
            '\\' => match it.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                'u' => {
                    let h: String = it.by_ref().take(4).collect();
                    out.push(u32::from_str_radix(&h, 16).ok().and_then(char::from_u32)?);
                }
                o => out.push(o),
            },
            c => out.push(c),
        }
    }
    None
}

fn json_labels(body: &str) -> Vec<String> {
    let Some(at) = body.find("\"labels\"") else {
        return Vec::new();
    };
    let rest = &body[at..];
    let (Some(a), Some(b)) = (rest.find('['), rest.find(']')) else {
        return Vec::new();
    };
    rest[a + 1..b]
        .split(',')
        .map(|s| s.trim().trim_matches('"').to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn fault_reply(f: &Fault) -> Response<std::io::Cursor<Vec<u8>>> {
    match f {
        Fault::Status(code, msg) => json_reply(
            *code,
            &format!(
                "{{\"message\":{},\"documentation_url\":\"https://docs.github.com/rest\"}}",
                esc(msg)
            ),
            &[],
        ),
        Fault::RateLimit(code) => json_reply(
            *code,
            "{\"message\":\"API rate limit exceeded for this token.\",\"documentation_url\":\"https://docs.github.com/rest/rate-limit\"}",
            &[
                ("x-ratelimit-limit", "30".into()),
                ("x-ratelimit-remaining", "0".into()),
                ("x-ratelimit-reset", "4102444800".into()),
                ("retry-after", "60".into()),
            ],
        ),
        Fault::AnswerAfterWrite(code) => json_reply(
            *code,
            "{\"message\":\"Server Error\"}",
            &[],
        ),
    }
}

fn take_fault(s: &mut State, e: Endpoint) -> Option<Fault> {
    for key in [e, Endpoint::Any] {
        if let Some((f, n)) = s.faults.get_mut(&key) {
            if *n > 0 {
                *n -= 1;
                return Some(f.clone());
            }
        }
    }
    None
}

fn handle(mut req: tiny_http::Request, state: &Shared, base: &str) {
    let full = req.url().to_string();
    let (path, query) = match full.split_once('?') {
        Some((p, q)) => (p.to_string(), Some(q.to_string())),
        None => (full.clone(), None),
    };
    let mut body = String::new();
    let _ = req.as_reader().take(512 * 1024).read_to_string(&mut body);
    let rec = TrackerRequest {
        method: req.method().as_str().to_string(),
        path: path.clone(),
        query,
        headers: req
            .headers()
            .iter()
            .map(|h| {
                (
                    h.field.as_str().as_str().to_string(),
                    h.value.as_str().to_string(),
                )
            })
            .collect(),
        body: body.clone(),
    };
    let mut s = lock(state);
    s.log.push(rec.clone());
    let reply = route(&mut s, &rec, &body, base);
    drop(s);
    let _ = req.respond(reply);
}

fn route(
    s: &mut State,
    rec: &TrackerRequest,
    body: &str,
    base: &str,
) -> Response<std::io::Cursor<Vec<u8>>> {
    if let Some(tok) = &s.token {
        let want = format!("Bearer {tok}");
        if rec.header("authorization") != Some(want.as_str()) {
            return json_reply(401, "{\"message\":\"Bad credentials\"}", &[]);
        }
    }
    let segs: Vec<&str> = rec.path.trim_matches('/').split('/').collect();
    let m = rec.method.as_str();
    match (m, segs.as_slice()) {
        ("GET", ["search", "issues"]) => {
            if let Some(f) = take_fault(s, Endpoint::Search) {
                return fault_reply(&f);
            }
            let q = rec.q().unwrap_or_default();
            if !q.split_whitespace().any(|w| w == "is:issue")
                || !q.split_whitespace().any(|w| w.starts_with("repo:"))
            {
                return json_reply(
                    422,
                    "{\"message\":\"Validation Failed\",\"errors\":[{\"message\":\"Query must include 'is:issue' or 'is:pull-request'\",\"resource\":\"Search\",\"field\":\"q\",\"code\":\"invalid\"}],\"documentation_url\":\"https://docs.github.com/v3/search/\"}",
                    &[],
                );
            }
            let repo = q
                .split_whitespace()
                .find_map(|w| w.strip_prefix("repo:"))
                .unwrap_or("")
                .to_string();
            let words: Vec<String> = q
                .split_whitespace()
                .filter(|w| !w.contains(':'))
                .map(|w| w.trim_matches('"').to_lowercase())
                .collect();
            let matches = |i: &TrackerIssue| {
                let hay = format!("{} {}", i.title, i.body).to_lowercase();
                words.iter().all(|w| hay.contains(w))
            };
            let mut hits: Vec<String> = s
                .issues
                .iter()
                .filter(|i| i.repo == repo)
                .filter(|i| matches(i))
                .map(|i| issue_json(i, base))
                .collect();
            // the search index lags a transfer: the old address still lists the issue
            for ((from, n), (to, m)) in &s.moved {
                if *from != repo {
                    continue;
                }
                if let Some(i) = s.issues.iter().find(|i| i.repo == *to && i.number == *m) {
                    if matches(i) {
                        let mut old = i.clone();
                        old.repo = from.clone();
                        old.number = *n;
                        hits.push(issue_json(&old, base));
                    }
                }
            }
            json_reply(
                200,
                &format!(
                    "{{\"total_count\":{},\"incomplete_results\":false,\"items\":[{}]}}",
                    hits.len(),
                    hits.join(",")
                ),
                &[("x-ratelimit-remaining", "29".into())],
            )
        }
        ("POST", ["repos", o, r, "issues"]) => {
            let repo = format!("{o}/{r}");
            let fault = take_fault(s, Endpoint::Create);
            if let Some(f) = &fault {
                if !matches!(f, Fault::AnswerAfterWrite(_)) {
                    return fault_reply(f);
                }
            }
            let title = json_field(body, "title").unwrap_or_default();
            if title.is_empty() {
                return json_reply(
                    422,
                    "{\"message\":\"Validation Failed\",\"errors\":[{\"resource\":\"Issue\",\"code\":\"missing_field\",\"field\":\"title\"}]}",
                    &[],
                );
            }
            let n = next_number(s, &repo);
            let issue = TrackerIssue {
                repo: repo.clone(),
                number: n,
                title,
                body: json_field(body, "body").unwrap_or_default(),
                labels: json_labels(body),
                comments: Vec::new(),
            };
            let out = issue_json(&issue, base);
            s.issues.push(issue);
            match fault {
                Some(f) => fault_reply(&f),
                None => json_reply(201, &out, &[]),
            }
        }
        ("GET", ["repos", o, r, "issues", n]) => {
            if let Some(f) = take_fault(s, Endpoint::Get) {
                return fault_reply(&f);
            }
            let repo = format!("{o}/{r}");
            let num: u64 = n.parse().unwrap_or(0);
            if let Some((r2, n2)) = s.moved.get(&(repo.clone(), num)) {
                let to = format!("{base}/repos/{r2}/issues/{n2}");
                return json_reply(
                    301,
                    "{\"message\":\"Moved Permanently\"}",
                    &[("Location", to)],
                );
            }
            match s.issues.iter().find(|i| i.repo == repo && i.number == num) {
                Some(i) => json_reply(200, &issue_json(i, base), &[]),
                None => json_reply(404, "{\"message\":\"Not Found\"}", &[]),
            }
        }
        ("POST", ["repos", o, r, "issues", n, "comments"]) => {
            let fault = take_fault(s, Endpoint::Comment);
            if let Some(f) = &fault {
                if !matches!(f, Fault::AnswerAfterWrite(_)) {
                    return fault_reply(f);
                }
            }
            let repo = format!("{o}/{r}");
            let num: u64 = n.parse().unwrap_or(0);
            if s.moved.contains_key(&(repo.clone(), num)) {
                let (r2, n2) = s.moved[&(repo, num)].clone();
                let to = format!("{base}/repos/{r2}/issues/{n2}/comments");
                return json_reply(
                    301,
                    "{\"message\":\"Moved Permanently\"}",
                    &[("Location", to)],
                );
            }
            match s
                .issues
                .iter_mut()
                .find(|i| i.repo == repo && i.number == num)
            {
                Some(i) => {
                    i.comments
                        .push(json_field(body, "body").unwrap_or_default());
                    match fault {
                        Some(f) => fault_reply(&f),
                        None => json_reply(201, "{\"id\":1}", &[]),
                    }
                }
                None => json_reply(404, "{\"message\":\"Not Found\"}", &[]),
            }
        }
        _ => json_reply(404, "{\"message\":\"Not Found\"}", &[]),
    }
}
