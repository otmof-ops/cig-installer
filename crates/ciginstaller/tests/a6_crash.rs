// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! The crash pipeline's installer side (#10, #28 part 1): the installer's own panic lands in
//! cig's ashtray in cig's schema; `crash list|show|send|delete|submit` work over every tool's
//! reports; `send` honors `reports` and the allowlist and carries no identifiers (#19); and a
//! person's own words survive the anonymiser.
//!
//! The issues route is a local fake: a `tiny_http` server that records what it was sent.

use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::{Arc, Mutex};

use cigi_testkit::Fixture;
use serde_json::{json, Value};

const USER: &str = "tessa-q";
const HOST: &str = "bunnyhutch";

fn fixture() -> Fixture {
    Fixture::new()
}

/// The installer with a scratch home, a user name and a host name to be kept out of reports,
/// and nothing about reports set (CI is not set either: `ask`).
fn cigi(fx: &Fixture, env: &[(&str, &str)], args: &[&str]) -> Output {
    let mut c = fx.cigi();
    c.env("USER", USER).env("HOSTNAME", HOST);
    for (k, v) in env {
        c.env(k, v);
    }
    c.args(args);
    cigi_testkit::output_retrying(&mut c).expect("run ciginstaller")
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn json(o: &Output) -> Value {
    let line = String::from_utf8_lossy(&o.stdout)
        .lines()
        .last()
        .unwrap_or("")
        .to_string();
    serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {}", text(o)))
}

fn ashtray(fx: &Fixture) -> PathBuf {
    fx.cigscript().join("ashtray")
}

fn files(fx: &Fixture) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(ashtray(fx))
        .map(|d| {
            d.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "json"))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

fn read(p: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

/// A report as another product of the family hands it in.
fn submit(fx: &Fixture, env: &[(&str, &str)], origin: &str, body: &Value) -> Value {
    let f = fx.user_home.join("handed-in.json");
    std::fs::write(&f, body.to_string()).unwrap();
    let o = cigi(
        fx,
        env,
        &[
            "--json",
            "crash",
            "submit",
            "--origin",
            origin,
            "--file",
            f.to_str().unwrap(),
        ],
    );
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    json(&o)["data"].clone()
}

// ---- the fake route ----------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Seen {
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    body: String,
}

/// What a fake route answers: a status, a body and one extra header.
type Answer = (u16, &'static str, Option<(&'static str, &'static str)>);

struct Route {
    base: String,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Route {
    /// Answers each request with the next of `answers` (status, body, extra header), the last
    /// one for every request after.
    fn start(answers: Vec<Answer>) -> Route {
        let server = tiny_http::Server::http("127.0.0.1:0").expect("bind a fake route");
        let port = server.server_addr().to_ip().expect("ip").port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        std::thread::spawn(move || {
            for (n, mut req) in server.incoming_requests().enumerate() {
                let mut body = String::new();
                let _ = req.as_reader().read_to_string(&mut body);
                log.lock().unwrap().push(Seen {
                    method: req.method().to_string(),
                    url: req.url().to_string(),
                    headers: req
                        .headers()
                        .iter()
                        .map(|h| {
                            (
                                h.field.to_string().to_ascii_lowercase(),
                                h.value.to_string(),
                            )
                        })
                        .collect(),
                    body,
                });
                let (code, text, extra) = answers[n.min(answers.len() - 1)];
                let mut resp = tiny_http::Response::from_string(text).with_status_code(code);
                if let Some((k, v)) = extra {
                    resp = resp.with_header(tiny_http::Header::from_bytes(k, v).unwrap());
                }
                let _ = req.respond(resp);
            }
        });
        Route {
            base: format!("http://127.0.0.1:{port}"),
            seen,
        }
    }
    fn url(&self) -> String {
        format!("{}/reports", self.base)
    }
    fn requests(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

const FILED: &str = r#"{"html_url":"https://github.com/otmof-ops/cig-installer/issues/41"}"#;

fn tricky(fx: &Fixture) -> Value {
    let home = fx.user_home.to_string_lossy().to_string();
    json!({
        "kind": "crash",
        "version": "1.20.0",
        "panic_message": format!("cannot read {home}/projects/{USER}-notes/plan.txt: nope (user {USER} on {HOST})"),
        "panic_location": "src/run.rs:10:3",
        "command": ["/usr/bin/cig", "run", format!("{home}/projects/x.cig"), "--token=ghp_abcdefghijklmnopqrstuvwxyz0123456789"],
        "cwd": format!("{home}/projects"),
        "os": "linux", "arch": "x86_64",
        "backtrace": [format!("cig::run (src/run.rs:10) at {home}/cig/src/run.rs")],
        "env": { "PATH": format!("{home}/bin:/usr/bin"), "TOKEN": "ghp_abcdefghijklmnopqrstuvwxyz0123456789" },
        "contact": "someone@example.com from 192.168.1.20",
    })
}

// ---- the installer's own panic ----------------------------------------------------------

#[cfg(debug_assertions)]
#[test]
fn the_installers_own_panic_lands_in_the_ashtray_in_cigs_schema() {
    let fx = fixture();
    let home = fx.user_home.to_string_lossy().to_string();
    let msg = format!("no such file {home}/proj/secret-{USER}.cig at byte 12 on {HOST}");
    let o = cigi(&fx, &[("CIGI_PLANT_PANIC", &msg)], &["status"]);
    assert_ne!(
        o.status.code(),
        Some(0),
        "a panic is not a success: {}",
        text(&o)
    );
    let t = text(&o);
    assert!(t.contains("E1890"), "{t}");
    assert!(t.contains("nothing was sent"), "{t}");
    let f = files(&fx);
    assert_eq!(f.len(), 1, "{t}");
    let r = read(&f[0]);
    assert_eq!(r["origin"], "ciginstaller");
    assert_eq!(r["kind"], "crash");
    // cig's schema: every field cig's reader requires is there
    for k in [
        "id",
        "fingerprint",
        "version",
        "os",
        "arch",
        "timestamp",
        "command",
        "cwd",
        "panic_message",
        "panic_location",
        "backtrace",
    ] {
        assert!(!r[k].is_null(), "{k} missing in {r}");
    }
    assert_eq!(r["fingerprint"].as_str().unwrap().len(), 8);
    assert!(
        r["panic_location"]
            .as_str()
            .unwrap()
            .starts_with("crates/ciginstaller/src/"),
        "{r}"
    );
    // nothing of the machine is in the file
    let file = std::fs::read_to_string(&f[0]).unwrap();
    for secret in [home.as_str(), USER, HOST, fx.cigscript().to_str().unwrap()] {
        assert!(!file.contains(secret), "{secret} is in the report:\n{file}");
    }
    assert!(
        r["panic_message"]
            .as_str()
            .unwrap()
            .contains("no such file"),
        "{r}"
    );
    // the queue shows it, and nothing was sent
    let l = json(&cigi(&fx, &[], &["--json", "crash", "list"]));
    assert_eq!(l["data"]["reports"][0]["origin"], "ciginstaller");
    assert_eq!(l["data"]["reports"][0]["state"], "local");
}

#[cfg(debug_assertions)]
#[test]
fn the_panic_hook_speaks_json_under_json() {
    let fx = fixture();
    let o = cigi(&fx, &[("CIGI_PLANT_PANIC", "boom")], &["--json", "status"]);
    assert!(
        o.stderr.is_empty(),
        "nothing on stderr under --json: {}",
        text(&o)
    );
    let v = json(&o);
    assert_eq!(v["ok"], false);
    assert_eq!(v["error"]["code"], "E1890");
    assert_eq!(files(&fx).len(), 1);
}

// ---- the fingerprint ---------------------------------------------------------------------

#[cfg(debug_assertions)]
#[test]
fn the_fingerprint_is_stable_for_the_same_fault_and_carries_no_identifier() {
    let a = fixture();
    let b = fixture();
    let ha = a.user_home.to_string_lossy().to_string();
    let hb = b.user_home.to_string_lossy().to_string();
    cigi(
        &a,
        &[(
            "CIGI_PLANT_PANIC",
            &format!("cannot read {ha}/one/two/plan.cig at byte 12"),
        )],
        &["status"],
    );
    cigi(
        &b,
        &[(
            "CIGI_PLANT_PANIC",
            &format!("cannot read {hb}/x/plan.cig at byte 9001"),
        )],
        &["status"],
    );
    let fa = read(&files(&a)[0])["fingerprint"]
        .as_str()
        .unwrap()
        .to_string();
    let fb = read(&files(&b)[0])["fingerprint"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        fa, fb,
        "the same fault in differently named folders is one fingerprint"
    );
    assert!(fa.bytes().all(|c| c.is_ascii_hexdigit()) && fa.len() == 8);
    cigi(
        &a,
        &[("CIGI_PLANT_PANIC", "a different fault")],
        &["status"],
    );
    let all: Vec<String> = files(&a)
        .iter()
        .map(|p| read(p)["fingerprint"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(all.len(), 2);
    assert_ne!(all[0], all[1], "another fault is another fingerprint");
}

// ---- own words ---------------------------------------------------------------------------

#[test]
fn own_words_survive_anonymising() {
    let fx = fixture();
    let home = fx.user_home.to_string_lossy().to_string();
    let cig_home = fx.cigscript().to_string_lossy().to_string();
    let lock = format!("{cig_home}/ashtray/.flush.lock");
    let body = json!({
        "kind": "internal",
        "version": "1.20.0",
        "panic_message": format!("could not take {lock}: held by pid 4242; see tin/statcache and orchestrate.cig, read {home}/work/{USER}/budget.xlsx and/or retry"),
        "panic_location": "E900 src/ashtray.rs",
        "own_paths": ["tin/statcache", "orchestrate.cig", "/home/other/folder"],
    });
    let d = submit(&fx, &[], "cig", &body);
    let id = d["id"].as_str().unwrap().to_string();
    let shown = json(&cigi(&fx, &[], &["--json", "crash", "show", &id]));
    let r = &shown["data"]["report"];
    let msg = r["panic_message"].as_str().unwrap();
    let title = r["title"].as_str().unwrap();
    // the family's own paths, readable, in the body and in the title (cig-script#127)
    assert!(msg.contains("$CIGSCRIPT_HOME/ashtray/.flush.lock"), "{msg}");
    assert!(msg.contains("tin/statcache"), "{msg}");
    assert!(msg.contains("orchestrate.cig"), "{msg}");
    assert!(
        title.contains("$CIGSCRIPT_HOME/ashtray/.flush.lock"),
        "{title}"
    );
    assert!(!msg.contains("/d1/d2/d3/.f1.lock"), "{msg}");
    // plain words, `and/or` and the numbers of a message are untouched
    assert!(
        msg.contains("could not take")
            && msg.contains("held by pid 4242")
            && msg.contains("and/or retry"),
        "{msg}"
    );
    // the person's own are shapes, and the user name is gone
    assert!(msg.contains("~/d1/d2/f3.xlsx"), "{msg}");
    for secret in [home.as_str(), USER, "budget"] {
        assert!(
            !msg.contains(secret) && !title.contains(secret),
            "{secret} in {msg}"
        );
    }
    // a product cannot register a person's folder as its own
    let again = submit(
        &fx,
        &[],
        "cig",
        &json!({
            "panic_message": "gone: /home/other/folder/data.txt",
            "version": "1.20.0",
            "own_paths": ["/home/other/folder"],
        }),
    );
    let s = json(&cigi(
        &fx,
        &[],
        &["--json", "crash", "show", again["id"].as_str().unwrap()],
    ));
    assert!(
        !s["data"]["report"]["panic_message"]
            .as_str()
            .unwrap()
            .contains("other"),
        "{s}"
    );
}

// ---- the ashtray is cig's -----------------------------------------------------------------

#[test]
fn a_report_cig_wrote_is_listed_and_survives_a_send_unchanged() {
    let fx = fixture();
    std::fs::create_dir_all(ashtray(&fx)).unwrap();
    let cig_report = json!({
        "id": "20261109T101500-abc123", "fingerprint": "abc12345", "version": "1.20.0", "os": "linux", "arch": "x86_64",
        "timestamp": "2026-11-09T10:15:00Z", "command": ["cig", "run", "<script>"], "cwd": "~/d1",
        "panic_message": "index out of range", "panic_location": "src/run.rs:1:1", "backtrace": [],
        "kind": "crash", "origin": "cig", "a_field_from_the_future": {"keep": true}
    });
    std::fs::write(
        ashtray(&fx).join("20261109T101500-abc123.json"),
        cig_report.to_string(),
    )
    .unwrap();
    // an older directory, and a file that is not a report
    std::fs::create_dir_all(fx.cigscript().join("crashes")).unwrap();
    std::fs::write(
        fx.cigscript().join("crashes/20260101T000000-old000.json"),
        cig_report.to_string().replace("abc123", "old000"),
    )
    .unwrap();
    std::fs::write(ashtray(&fx).join("stray.json"), "{not json").unwrap();
    std::fs::write(ashtray(&fx).join(".flush.lock"), "").unwrap();
    let l = json(&cigi(&fx, &[], &["--json", "crash", "list"]));
    assert_eq!(l["data"]["reports"].as_array().unwrap().len(), 2, "{l}");
    assert_eq!(l["data"]["unreadable"], 1);
    assert_eq!(l["data"]["reports"][0]["origin"], "cig");
    assert_eq!(l["data"]["reports"][0]["target"], "otmof-ops/cig-script");
    let route = Route::start(vec![(200, FILED, None)]);
    let o = cigi(
        &fx,
        &[("CIGI_REPORT_ROUTE", &route.url())],
        &["--json", "crash", "send", "20261109T101500-abc123"],
    );
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    let after = read(&ashtray(&fx).join("20261109T101500-abc123.json"));
    assert_eq!(
        after["a_field_from_the_future"],
        json!({"keep": true}),
        "a field this build does not know is kept"
    );
    assert_eq!(after["issue"]["number"], 41);
    assert_eq!(after["issue"]["repo"], "otmof-ops/cig-installer");
    assert!(after["sent"].is_string());
    // the travelling object has no field it does not own
    let sent: Value = serde_json::from_str(&route.requests()[0].body).unwrap();
    assert!(
        sent.get("a_field_from_the_future").is_none()
            && sent.get("queue").is_none()
            && sent.get("issue").is_none(),
        "{sent}"
    );
}

// ---- send ---------------------------------------------------------------------------------

fn schema_keys() -> Vec<String> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schemas/report.schema.json");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
    v["properties"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect()
}

#[test]
fn send_carries_exactly_what_show_prints_and_no_identifier() {
    let fx = fixture();
    let home = fx.user_home.to_string_lossy().to_string();
    let d = submit(&fx, &[], "cig", &tricky(&fx));
    let id = d["id"].as_str().unwrap().to_string();
    let shown = json(&cigi(&fx, &[], &["--json", "crash", "show", &id]));
    let route = Route::start(vec![(201, FILED, None)]);
    let env = [("CIGI_REPORT_ROUTE", route.url())];
    let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let o = cigi(&fx, &env, &["--json", "crash", "send", &id]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    let reqs = route.requests();
    assert_eq!(reqs.len(), 1);
    let r = &reqs[0];
    assert_eq!(r.method, "POST");
    assert_eq!(
        r.url, "/reports",
        "no query string, no identifiers in the address"
    );
    // the body is the shown object, no more and no less
    let body: Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(body, shown["data"]["report"]);
    let allowed = schema_keys();
    for k in body.as_object().unwrap().keys() {
        assert!(allowed.contains(k), "{k} is not in the allowlist");
    }
    // #19: nothing identifies the person or the machine, in the body or in the request
    for secret in [
        home.as_str(),
        USER,
        HOST,
        "ghp_",
        "someone@example.com",
        "192.168.1.20",
        fx.cigscript().to_str().unwrap(),
    ] {
        assert!(!r.body.contains(secret), "{secret} travelled: {}", r.body);
        assert!(!r.url.contains(secret));
    }
    let names: Vec<&str> = r.headers.iter().map(|(k, _)| k.as_str()).collect();
    for banned in [
        "authorization",
        "cookie",
        "proxy-authorization",
        "x-forwarded-for",
        "referer",
        "from",
    ] {
        assert!(!names.contains(&banned), "{banned} was sent: {names:?}");
    }
    for n in &names {
        assert!(
            [
                "host",
                "user-agent",
                "content-type",
                "content-length",
                "accept",
                "accept-encoding"
            ]
            .contains(n),
            "unexpected header {n}"
        );
    }
    let ua = r
        .headers
        .iter()
        .find(|(k, _)| k == "user-agent")
        .map(|(_, v)| v.as_str());
    assert_eq!(ua, Some("ciginstaller"));
    assert!(body["command"]
        .as_array()
        .unwrap()
        .iter()
        .all(|c| !c.as_str().unwrap().contains("ghp_")));
    // it is filed once: the second send does not send again
    let again = cigi(&fx, &env, &["--json", "crash", "send", &id]);
    assert_eq!(again.status.code(), Some(0), "{}", text(&again));
    assert_eq!(
        route.requests().len(),
        1,
        "a filed report is not sent twice"
    );
    assert_eq!(
        json(&cigi(&fx, &[], &["--json", "crash", "list"]))["data"]["reports"][0]["state"],
        "filed"
    );
}

#[test]
fn reports_never_is_honored_by_every_way_of_saying_it() {
    for (what, env) in [
        ("CIG_REPORTS", vec![("CIG_REPORTS", "never")]),
        ("CIGI_REPORTS", vec![("CIGI_REPORTS", "never")]),
        (
            "CIG_REPORTS wins over a file but never beats never",
            vec![("CIG_REPORTS", "always"), ("CIGI_REPORTS", "never")],
        ),
    ] {
        let fx = fixture();
        let d = submit(&fx, &env, "cig", &tricky(&fx));
        let id = d["id"].as_str().unwrap().to_string();
        let route = Route::start(vec![(200, FILED, None)]);
        let mut e = env.clone();
        let url = route.url();
        e.push(("CIGI_REPORT_ROUTE", url.as_str()));
        let o = cigi(&fx, &e, &["--json", "crash", "send", &id]);
        let t = text(&o);
        assert_eq!(o.status.code(), Some(2), "{what}: {t}");
        assert!(t.contains("E1857") && t.contains("never"), "{what}: {t}");
        assert!(route.requests().is_empty(), "{what}: nothing may leave");
        assert!(!files(&fx).is_empty(), "{what}: the report stays");
        // `send --all` holds too
        let all = cigi(&fx, &e, &["crash", "send", "--all"]);
        assert_eq!(all.status.code(), Some(2), "{what}: {}", text(&all));
        assert!(route.requests().is_empty(), "{what}");
    }
}

#[test]
fn under_ci_with_nothing_set_a_submitted_report_stays_local_and_is_not_queued() {
    let fx = fixture();
    // CI with no terminal and no setting is never (as cig's): saved, not queued
    let d = submit(
        &fx,
        &[("CI", "true")],
        "cig-orchestrator",
        &json!({"panic_message": "x", "version": "1.17.0"}),
    );
    let l = json(&cigi(&fx, &[("CI", "true")], &["--json", "crash", "list"]));
    assert_eq!(l["data"]["reports_setting"], "never");
    assert_eq!(l["data"]["reports"][0]["state"], "local");
    assert_eq!(
        l["data"]["reports"][0]["target"],
        "otmof-ops/cig-orchestrator"
    );
    assert!(d["id"].is_string());
}

#[test]
fn with_no_route_the_report_is_kept_and_the_output_says_so() {
    let fx = fixture();
    let d = submit(
        &fx,
        &[],
        "cig-compile",
        &json!({"panic_message": "boom", "version": "0.2.0"}),
    );
    let id = d["id"].as_str().unwrap().to_string();
    let o = cigi(&fx, &[], &["crash", "send", &id]);
    let t = text(&o);
    assert_eq!(o.status.code(), Some(0), "{t}");
    assert!(
        t.contains("nothing was sent") && t.contains("no issues route yet"),
        "{t}"
    );
    assert!(
        t.contains("#28") && t.contains("#24"),
        "the blockers are named: {t}"
    );
    assert!(t.contains("stays queued"), "{t}");
    assert_eq!(files(&fx).len(), 1);
    let l = json(&cigi(&fx, &[], &["--json", "crash", "list"]));
    assert_eq!(l["data"]["reports"][0]["state"], "pending");
    assert_eq!(l["data"]["route"]["configured"], false);
    // a route that is not safe is refused with its reason
    let bad = cigi(
        &fx,
        &[("CIGI_REPORT_ROUTE", "http://example.com/r")],
        &["crash", "send", &id],
    );
    assert_eq!(bad.status.code(), Some(1), "{}", text(&bad));
    assert!(
        text(&bad).contains("E1857") && text(&bad).contains("loopback"),
        "{}",
        text(&bad)
    );
}

#[test]
fn a_route_that_fails_leaves_the_report_queued_with_the_reason_then_files_it_once() {
    let fx = fixture();
    let d = submit(&fx, &[], "cig", &tricky(&fx));
    let id = d["id"].as_str().unwrap().to_string();
    // 500, then a redirect that is never followed, then success
    let route = Route::start(vec![
        (500, "no", None),
        (302, "", Some(("Location", "http://127.0.0.1:9/elsewhere"))),
        (200, FILED, None),
    ]);
    let url = route.url();
    let env = [("CIGI_REPORT_ROUTE", url.as_str())];
    for (n, expect) in [(1, "500"), (2, "302")] {
        let o = cigi(&fx, &env, &["crash", "send", &id]);
        let t = text(&o);
        assert_eq!(o.status.code(), Some(1), "{t}");
        assert!(
            t.contains("E1857") && t.contains(expect) && t.contains("stays queued"),
            "{t}"
        );
        assert_eq!(route.requests().len(), n);
        let l = json(&cigi(&fx, &[], &["--json", "crash", "list"]));
        assert!(
            l["data"]["reports"][0]["state"]
                .as_str()
                .unwrap()
                .starts_with("failed"),
            "{l}"
        );
        assert!(l["data"]["reports"][0]["last_error"]
            .as_str()
            .unwrap()
            .contains(expect));
    }
    let o = cigi(&fx, &env, &["crash", "send", &id]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    assert_eq!(route.requests().len(), 3);
    assert!(
        route.requests().iter().all(|r| r.url == "/reports"),
        "the redirect target was never asked"
    );
    cigi(&fx, &env, &["crash", "send", &id]);
    assert_eq!(route.requests().len(), 3, "filed exactly once");
    assert_eq!(read(&files(&fx)[0])["queue"]["last_error"], Value::Null);
}

#[test]
fn a_dead_route_is_a_queued_report_not_a_lost_one() {
    let fx = fixture();
    let d = submit(&fx, &[], "cig", &tricky(&fx));
    let id = d["id"].as_str().unwrap().to_string();
    let o = cigi(
        &fx,
        &[("CIGI_REPORT_ROUTE", "http://127.0.0.1:9/reports")],
        &["crash", "send", &id],
    );
    let t = text(&o);
    assert_eq!(o.status.code(), Some(1), "{t}");
    assert!(
        t.contains("could not be reached") && t.contains("stays queued"),
        "{t}"
    );
    assert_eq!(files(&fx).len(), 1);
}

// ---- submit, delete, list ---------------------------------------------------------------

#[test]
fn submit_names_its_origin_and_repo() {
    let fx = fixture();
    let f = fx.user_home.join("r.json");
    std::fs::write(&f, tricky(&fx).to_string()).unwrap();
    let o = cigi(
        &fx,
        &[],
        &[
            "crash",
            "submit",
            "--origin",
            "cig orchestrator",
            "--file",
            f.to_str().unwrap(),
        ],
    );
    assert_eq!(o.status.code(), Some(3), "{}", text(&o));
    let o = cigi(
        &fx,
        &[],
        &[
            "crash",
            "submit",
            "--origin",
            "cigcompile",
            "--repo",
            "nope",
            "--file",
            f.to_str().unwrap(),
        ],
    );
    assert_eq!(o.status.code(), Some(3), "{}", text(&o));
    let d = submit(&fx, &[], "cigar-script", &tricky(&fx));
    let l = json(&cigi(
        &fx,
        &[],
        &["--json", "crash", "show", d["id"].as_str().unwrap()],
    ));
    // the top frame is `cig::run`: the fault is cig's, so it is filed there (#28 section 3: blame,
    // not only origin); the report still says where it was seen
    assert_eq!(l["data"]["target"], "otmof-ops/cig-script");
    assert_eq!(l["data"]["report"]["origin"], "cigar-script");
    // the command line is the words the installer knows; the person's script is a shape
    let cmd = l["data"]["report"]["command"].to_string();
    assert!(!cmd.contains("projects") && !cmd.contains("ghp_"), "{cmd}");
}

#[test]
fn delete_by_prefix_and_all() {
    let fx = fixture();
    let a = submit(
        &fx,
        &[],
        "cig",
        &json!({"panic_message": "one", "version": "1.0.0"}),
    );
    submit(
        &fx,
        &[],
        "cig",
        &json!({"panic_message": "two", "version": "1.0.0"}),
    );
    assert_eq!(files(&fx).len(), 2);
    let id = a["id"].as_str().unwrap();
    // a prefix that fits both is ambiguous and deletes nothing
    let o = cigi(&fx, &[], &["crash", "delete", "2026"]);
    assert_eq!(o.status.code(), Some(3), "{}", text(&o));
    assert_eq!(files(&fx).len(), 2);
    let o = cigi(&fx, &[], &["crash", "delete", id]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    assert_eq!(files(&fx).len(), 1);
    // --all is irreversible: with no terminal and no flag it asks and stops
    let o = cigi(&fx, &[], &["crash", "delete", "--all"]);
    assert_eq!(o.status.code(), Some(3), "{}", text(&o));
    assert_eq!(files(&fx).len(), 1);
    let o = cigi(
        &fx,
        &[],
        &["--yes-irreversible", "crash", "delete", "--all"],
    );
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    assert!(files(&fx).is_empty());
    let l = cigi(&fx, &[], &["crash"]);
    assert!(text(&l).contains("ashtray is empty"), "{}", text(&l));
}
