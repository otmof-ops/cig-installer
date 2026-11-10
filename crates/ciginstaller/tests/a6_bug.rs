// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `ciginstaller bug` (#29 step 1): the answers stay the person's words, the evidence for a run
//! carries a verified repro, the preview is the saved object, the version matrix cannot be
//! removed, `reports = never` sends nothing, and no request is made without a yes (#19).
//!
//! The products are the fixture's fakes (shell scripts), so these tests are Unix only. Nothing
//! is asked here (no terminal): the conversation itself is tested in `cmd/bug.rs` with a captured
//! UI. The fake cig answers `report-bundle` with a bundle that carries a verified repro, and
//! `--json runs` with one failed run.
#![cfg(unix)]

mod common;

use std::path::PathBuf;
use std::process::Output;
use std::sync::{Arc, Mutex};

use cigi_testkit::Fixture;
use common::{cigi, fixture, ok, text};
use serde_json::{json, Value};

const USER: &str = "tessa-q";
const HOST: &str = "bunnyhutch";

// ---- the fakes -----------------------------------------------------------------------------

fn bundle(with_repro: bool) -> String {
    let mut b = json!({
        "kind": "cigscript-report",
        "cig": "1.20.0",
        "run": {"id": "run-77", "status": "unburn_failed", "error": "unburn failed on a.txt"},
        "journal": [{"seq": 1, "op": "exhale", "path": "root0/a.txt"}, {"seq": 2, "op": "exhale", "path": "root0/a.txt"}],
        "diagnostic": {"code": "E501"},
    });
    if with_repro {
        b["repro"] = json!({
            "script": "exhale a.txt one\nexhale a.txt two\nunburn",
            "ops": 3, "verified": true, "expect": "a.txt is back as it was",
            "check": "tripwire:rollback", "note": "",
        });
    }
    b.to_string()
}

/// cig 1.20.0 with `report-bundle` and a list of runs; calls are logged to `$HOME/cig-calls`.
fn cig(fx: &mut Fixture, with_repro: bool) {
    let b = bundle(with_repro).replace('\'', "'\\''");
    let runs = json!([
        {"id": "run-70", "status": "ok", "started": "2026-11-06T10:00:00Z"},
        {"id": "run-77", "status": "unburn_failed", "started": "2026-11-06T18:47:24Z"}
    ])
    .to_string();
    fx.product("cig", "1.20.0").dry_run_output("plan: 1 op").shell(&format!(
        "echo \"$*\" >> \"$HOME/cig-calls\"\nif [ \"$1 $2\" = \"--json runs\" ]; then printf '%s' '{runs}'; exit 0; fi\nif [ \"$1\" = \"report-bundle\" ]; then if [ \"$2\" = \"run-77\" ]; then printf '%s' '{b}'; exit 0; fi; echo 'error[E803 usage]: no run matches' >&2; exit 3; fi\n"
    ));
}

fn machine(with_repro: bool) -> Fixture {
    let mut fx = fixture();
    cig(&mut fx, with_repro);
    fx.carton("0.1.0", &[("cig", "1.20.0")]);
    fx.publish("stable");
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    fx
}

fn run(fx: &Fixture, env: &[(&str, &str)], args: &[&str]) -> Output {
    let mut c = fx.cigi();
    c.env("CIGI_ALLOW_SINGLE_SOURCE", "true")
        .env("USER", USER)
        .env("HOSTNAME", HOST);
    for (k, v) in env {
        c.env(k, v);
    }
    c.args(args);
    cigi_testkit::output_retrying(&mut c).expect("run ciginstaller")
}

fn json_of(o: &Output) -> Value {
    let line = String::from_utf8_lossy(&o.stdout)
        .lines()
        .last()
        .unwrap_or("")
        .to_string();
    serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {}", text(o)))
}

/// `--json <args>` that must succeed; its `data`.
fn data(fx: &Fixture, env: &[(&str, &str)], args: &[&str]) -> Value {
    let mut a = vec!["--json"];
    a.extend_from_slice(args);
    let o = run(fx, env, &a);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    json_of(&o)["data"].clone()
}

fn ashtray(fx: &Fixture) -> Vec<PathBuf> {
    std::fs::read_dir(fx.cigscript().join("ashtray"))
        .map(|d| {
            d.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "json"))
                .collect()
        })
        .unwrap_or_default()
}

type Seen = (Vec<(String, String)>, String);

struct Route {
    base: String,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Route {
    fn start() -> Route {
        let server = tiny_http::Server::http("127.0.0.1:0").expect("bind a fake route");
        let port = server.server_addr().to_ip().expect("ip").port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        std::thread::spawn(move || {
            for mut req in server.incoming_requests() {
                let mut body = String::new();
                let _ = std::io::Read::read_to_string(req.as_reader(), &mut body);
                let headers = req
                    .headers()
                    .iter()
                    .map(|h| {
                        (
                            h.field.to_string().to_ascii_lowercase(),
                            h.value.to_string(),
                        )
                    })
                    .collect();
                log.lock().unwrap().push((headers, body));
                let r = tiny_http::Response::from_string(
                    r#"{"html_url":"https://github.com/otmof-ops/cig-script/issues/125"}"#,
                )
                .with_status_code(201);
                let _ = req.respond(r);
            }
        });
        Route {
            base: format!("http://127.0.0.1:{port}/reports"),
            seen,
        }
    }
    fn requests(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

// ---- #29's table ---------------------------------------------------------------------------

#[test]
fn free_text_marks_but_never_rewrites() {
    let fx = machine(false);
    let said = "edited /srv/plans/q3/notes.txt then mailed me@example.org, two  spaces stay";
    // untouched text is exactly what was typed
    let plain = data(&fx, &[], &["bug", "--did", "rolled back a run  twice"]);
    assert_eq!(
        plain["report"]["details"]["answers"]["did"],
        "rolled back a run  twice"
    );
    assert_eq!(plain["marks"], json!([]));

    // marked, listed, and replaced by default: nothing is rewritten without a line saying so
    let d = data(&fx, &[], &["bug", "--did", said, "--happened", "ok"]);
    let marks = d["marks"].as_array().unwrap();
    let kinds: Vec<&str> = marks.iter().map(|m| m["kind"].as_str().unwrap()).collect();
    assert!(
        kinds.contains(&"path") && kinds.contains(&"email"),
        "{marks:?}"
    );
    assert!(marks
        .iter()
        .all(|m| m["kept"] == false && m["field"] == "did"));
    let shown = d["report"]["details"]["answers"]["did"].as_str().unwrap();
    assert!(
        !shown.contains("/srv/plans") && !shown.contains("me@example.org"),
        "{shown}"
    );
    assert!(
        shown.contains("<email>") && shown.contains("two  spaces stay"),
        "{shown}"
    );
    let o = run(&fx, &[], &["bug", "--did", said]);
    assert!(
        text(&o).contains("/srv/plans/q3/notes.txt") && text(&o).contains("replaced with"),
        "{}",
        text(&o)
    );

    // kept only if the person says so, in the preview and in what is saved and shown again
    let k = data(&fx, &[], &["bug", "--did", said, "--keep-marked", "--save"]);
    assert!(k["bug"]["marks"]
        .as_array()
        .unwrap()
        .iter()
        .all(|m| m["kept"] == true));
    let kept = k["bug"]["report"]["details"]["answers"]["did"]
        .as_str()
        .unwrap();
    assert!(
        kept.contains("/srv/plans/q3/notes.txt") && kept.contains("me@example.org"),
        "{kept}"
    );
    let id = k["bug"]["id"].as_str().unwrap();
    let again = data(&fx, &[], &["crash", "show", id]);
    assert_eq!(again["report"]["details"]["answers"]["did"], kept);
    // the title is never the kept text
    assert!(!again["report"]["title"]
        .as_str()
        .unwrap()
        .contains("/srv/plans"));

    // a token is never kept, even when everything else is
    let tok = "sk-abcdefghijklmnopqrstuvwxyz0123456789";
    let t = data(
        &fx,
        &[],
        &[
            "bug",
            "--happened",
            &format!("used {tok} here"),
            "--keep-marked",
        ],
    );
    let m = &t["marks"][0];
    assert_eq!(
        (m["kind"].as_str(), m["kept"].as_bool()),
        (Some("token"), Some(false))
    );
    assert!(!t["report"].to_string().contains(tok));
}

#[test]
fn bug_for_a_run_attaches_a_verified_repro() {
    let fx = machine(true);
    let d = data(
        &fx,
        &[],
        &[
            "bug",
            "--run",
            "run-77",
            "--part",
            "cig",
            "--did",
            "rolled back a run with two hops on one file",
            "--expected",
            "the file back as it was",
            "--happened",
            "left at the first hop's version",
            "--save",
        ],
    );
    let r = &d["bug"]["report"];
    assert_eq!(r["details"]["repro_status"], "verified on this machine");
    assert_eq!(r["repro"]["verified"], true);
    assert!(r["repro"]["script"].as_str().unwrap().contains("a.txt"));
    // the bundle is in the run section, without the repro twice
    assert_eq!(
        r["details"]["run"]["record"]["cig_bundle"]["kind"],
        "cigscript-report"
    );
    assert!(r["details"]["run"]["record"]["cig_bundle"]
        .get("repro")
        .is_none());
    assert_eq!(
        r["details"]["answers"]["expected"],
        "the file back as it was"
    );
    // a verified repro asks no "every time" question and carries none
    assert!(r["details"]["answers"].get("every_time").is_none());
    // the evidence "always" row
    assert!(
        r["details"]["version_matrix"]["tools"]
            .as_array()
            .unwrap()
            .len()
            == 1
    );
    assert_eq!(r["details"]["environment"]["carton"], "0.1.0");
    assert_eq!(r["details"]["doctor"]["for"], "cig");
    assert_eq!(r["kind"], "report");
    assert_eq!(r["origin"], "ciginstaller");
    // the receipt
    let l = data(&fx, &[], &["bug", "list"]);
    assert_eq!(l["reports"][0]["n"], 1);
    assert_eq!(l["reports"][0]["part"], "cig");
    assert_eq!(l["reports"][0]["target"], "otmof-ops/cig-script");
    let s = data(&fx, &[], &["bug", "show", "1"]);
    assert_eq!(s["report"], *r);
}

#[test]
fn without_a_verified_repro_it_says_so_and_still_goes() {
    let fx = machine(false);
    let d = data(
        &fx,
        &[],
        &["bug", "--run", "run-77", "--every-time", "no", "--save"],
    );
    let r = &d["bug"]["report"];
    assert!(r["details"]["repro_status"]
        .as_str()
        .unwrap()
        .starts_with("no repro"));
    assert!(r.get("repro").is_none());
    assert_eq!(r["details"]["answers"]["every_time"], "no");
    // a product with no run is just as welcome
    let p = data(&fx, &[], &["bug", "--product", "cigo", "--save"]);
    assert_eq!(p["bug"]["report"]["details"]["part"], "cigo");
    assert_eq!(p["bug"]["report"]["details"]["doctor"]["for"], "cigo");
    assert!(p["bug"]["report"]["details"].get("run").is_none());
}

#[test]
fn not_sure_takes_the_part_from_the_run_and_last_finds_it() {
    let fx = machine(false);
    let d = data(&fx, &[], &["bug", "--part", "unsure", "--run", "run-77"]);
    assert_eq!(d["part"], "cig", "E501 is cig's");
    let l = data(&fx, &[], &["bug", "--last"]);
    assert_eq!(l["report"]["active_run_id"], "run-77");
    let none = run(&fx, &[], &["--json", "bug", "--run", "no-such-run"]);
    assert_ne!(none.status.code(), Some(0), "{}", text(&none));
    assert!(ashtray(&fx).is_empty());
}

// ---- the questions and the preview ---------------------------------------------------------

#[test]
fn every_question_is_skippable_without_a_terminal() {
    let fx = machine(false);
    // no flags, no terminal: nothing is asked, nothing is saved, the matrix is still there
    let d = data(&fx, &[], &["bug"]);
    assert_eq!(d["saved"], false);
    assert_eq!(d["report"]["details"]["answers"], json!({}));
    assert!(d["report"]["details"]["version_matrix"]["tools"].is_array());
    assert!(ashtray(&fx).is_empty());
    // --yes answers nothing and saves nothing
    let y = run(&fx, &[], &["--yes", "bug"]);
    assert_eq!(y.status.code(), Some(0), "{}", text(&y));
    assert!(text(&y).contains("nothing was saved"), "{}", text(&y));
    assert!(ashtray(&fx).is_empty());
}

#[test]
fn the_preview_is_what_is_saved_byte_for_byte() {
    let fx = machine(true);
    let d = data(
        &fx,
        &[],
        &[
            "bug",
            "--run",
            "run-77",
            "--did",
            "x",
            "--without",
            "doctor",
            "--save",
        ],
    );
    let id = d["bug"]["id"].as_str().unwrap().to_string();
    let shown = &d["bug"]["report"];
    let again = data(&fx, &[], &["crash", "show", &id]);
    assert_eq!(
        serde_json::to_string_pretty(shown).unwrap(),
        serde_json::to_string_pretty(&again["report"]).unwrap()
    );
    // and the text of `bug show` is the text of `crash show`'s object, section by section
    let t = run(&fx, &[], &["bug", "show", &id]);
    assert_eq!(t.status.code(), Some(0), "{}", text(&t));
    // take the `## ` section labels and the headline lines out: what is left is the object
    let body = String::from_utf8_lossy(&t.stdout).to_string();
    let shown_lines: Vec<String> = body
        .lines()
        .map(str::trim)
        .skip_while(|l| !l.starts_with('{'))
        .filter(|l| !l.starts_with("## ") && !l.starts_with('\u{2192}') && !l.starts_with("->"))
        .map(str::to_string)
        .collect();
    let want: Vec<String> = serde_json::to_string_pretty(shown)
        .unwrap()
        .lines()
        .map(|l| l.trim().to_string())
        .collect();
    assert_eq!(
        shown_lines, want,
        "the text preview is the object, line for line"
    );
    assert!(body.contains("## version matrix (always sent)"), "{body}");
    assert_eq!(ashtray(&fx).len(), 1);
}

#[test]
fn the_version_matrix_cannot_be_removed_and_the_rest_can() {
    let fx = machine(true);
    let o = run(
        &fx,
        &[],
        &["--json", "bug", "--without", "version-matrix", "--save"],
    );
    assert_ne!(o.status.code(), Some(0));
    assert!(
        text(&o).contains("version matrix cannot be removed"),
        "{}",
        text(&o)
    );
    assert!(ashtray(&fx).is_empty(), "nothing was saved");
    let d = data(
        &fx,
        &[],
        &[
            "bug",
            "--run",
            "run-77",
            "--without",
            "doctor",
            "--without",
            "run",
            "--without",
            "repro",
            "--without",
            "environment",
            "--did",
            "x",
            "--save",
        ],
    );
    let r = &d["bug"]["report"];
    let det = &r["details"];
    for gone in ["doctor", "run", "environment", "repro_status"] {
        assert!(det.get(gone).is_none(), "{gone} should be out: {det}");
    }
    assert!(r.get("repro").is_none());
    assert_eq!(
        det["removed"],
        json!(["doctor", "environment", "repro", "run"])
    );
    assert!(det["version_matrix"]["tools"].is_array());
    assert_eq!(det["answers"]["did"], "x");
}

// ---- consent, and no request without a yes (#19) -------------------------------------------

#[test]
fn reports_never_sends_nothing() {
    let fx = machine(true);
    let route = Route::start();
    for env in [("CIG_REPORTS", "never"), ("CIGI_REPORTS", "never")] {
        let o = run(
            &fx,
            &[("CIGI_REPORT_ROUTE", &route.base), env],
            &["--json", "bug", "--run", "run-77", "--did", "x", "--send"],
        );
        assert_eq!(o.status.code(), Some(2), "{}", text(&o));
        assert!(text(&o).contains("E1857"), "{}", text(&o));
        assert!(ashtray(&fx).is_empty(), "refused before anything was saved");
    }
    // keeping it here is not sending: it works under never
    let k = data(
        &fx,
        &[("CIGI_REPORT_ROUTE", &route.base), ("CIG_REPORTS", "never")],
        &["bug", "--did", "x", "--save"],
    );
    assert_eq!(k["saved"], true);
    assert_eq!(ashtray(&fx).len(), 1);
    assert!(route.requests().is_empty());
}

#[test]
fn no_network_request_without_a_yes() {
    let fx = machine(true);
    let route = Route::start();
    let env = [
        ("CIGI_REPORT_ROUTE", route.base.as_str()),
        ("CIG_REPORTS", "always"),
    ];
    // a draft, a save, the receipt and a show: the route sees nothing, whatever the consent says
    data(&fx, &env, &["bug", "--run", "run-77", "--did", "x"]);
    ok(&run(
        &fx,
        &env,
        &["--yes", "bug", "--run", "run-77", "--did", "x"],
    ));
    data(
        &fx,
        &env,
        &["bug", "--run", "run-77", "--did", "x", "--save"],
    );
    data(&fx, &env, &["bug", "list"]);
    data(&fx, &env, &["bug", "show", "1"]);
    assert!(
        route.requests().is_empty(),
        "a request was made without a yes"
    );
    assert_eq!(ashtray(&fx).len(), 1, "only the --save kept anything");

    // the yes: one POST of exactly what `crash show` prints, no identifier, nothing else
    let d = data(
        &fx,
        &env,
        &[
            "bug",
            "--run",
            "run-77",
            "--did",
            "wrote to me@example.org",
            "--send",
        ],
    );
    let reqs = route.requests();
    assert_eq!(reqs.len(), 1, "one POST");
    let (headers, body) = &reqs[0];
    let sent: Value = serde_json::from_str(body).unwrap();
    let id = d["bug"]["id"].as_str().unwrap();
    let shown = data(&fx, &env, &["crash", "show", id]);
    assert_eq!(
        sent, shown["report"],
        "what was sent is what the preview showed"
    );
    assert_eq!(sent["details"]["repro_status"], "verified on this machine");
    for secret in [
        USER,
        HOST,
        "me@example.org",
        fx.user_home.to_str().unwrap(),
        fx.cigscript().to_str().unwrap(),
    ] {
        assert!(!body.contains(secret), "{secret} left the machine:\n{body}");
    }
    for (k, _) in headers {
        assert!(
            !matches!(
                k.as_str(),
                "authorization" | "cookie" | "proxy-authorization"
            ),
            "{k}"
        );
    }
    assert!(
        d["sent"][0]["url"]
            .as_str()
            .unwrap()
            .ends_with("/issues/125"),
        "{d}"
    );
}
