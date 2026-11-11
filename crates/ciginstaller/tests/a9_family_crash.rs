// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! One ashtray for the family (#10): a crash in CigCompile and one in CigarScript's `cigar`, as
//! those tools write it into cig's ashtray, is shown by `ciginstaller crash list` and filed in the
//! tool's own repository; `reports = never` keeps both home; and the one answer the tools read
//! before they save anything (`config get reports --json`) has the shape they parse.
//!
//! The report files below are what the tools write (their own tests check that), written by hand
//! as fixtures. The tracker is a fake on 127.0.0.1; nothing here reaches a real one.

use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use cigi_testkit::{FakeTracker, Fixture};
use serde_json::{json, Value};

const TOKEN: &str = "tkn-fake-0123456789";
const CIG_COMPILE: &str = "otmof-ops/cig-compile";
const CIGAR: &str = "otmof-ops/cigar-script";

fn run(mut c: Command) -> Output {
    c.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = c.spawn().expect("start ciginstaller");
    let pid = child.id();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    match rx.recv_timeout(Duration::from_secs(90)) {
        Ok(Ok(o)) => o,
        _ => {
            let _ = Command::new("kill").args(["-9", &pid.to_string()]).status();
            panic!("ciginstaller did not finish in 90 seconds");
        }
    }
}

fn cigi(fx: &Fixture, env: &[(&str, &str)], args: &[&str]) -> Output {
    let mut c = fx.cigi();
    c.env("USER", "tessa-q").env("HOSTNAME", "bunnyhutch");
    for (k, v) in env {
        c.env(k, v);
    }
    c.args(args);
    run(c)
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn json_of(o: &Output) -> Value {
    let line = String::from_utf8_lossy(&o.stdout)
        .lines()
        .last()
        .unwrap_or("")
        .to_string();
    serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {}", text(o)))
}

fn code(o: &Output) -> i32 {
    o.status.code().unwrap_or(-1)
}

fn ashtray(fx: &Fixture) -> PathBuf {
    fx.cigscript().join("ashtray")
}

/// A crash report as `cigcompile` writes it (cig's schema, origin `cig-compile`).
fn cig_compile_report(repo: bool, frames: bool) -> Value {
    let mut r = json!({
        "id": "20261111T090000-c0c0c0", "fingerprint": "c0c0c0c0", "version": "0.3.0",
        "os": "linux", "arch": "x86_64", "timestamp": "2026-11-11T09:00:00Z",
        "command": ["cigcompile", "build", "<flag>", "<script>"], "cwd": "/d1/d2",
        "panic_message": "index out of bounds: the len is # but the index is #",
        "panic_location": "src/build.rs:30:9",
        "backtrace": [],
        "active_run_id": null, "sent": null, "rolled_back": null,
        "kind": "crash", "origin": "cig-compile",
        "queue": {"requested": false, "attempts": 0},
    });
    if repo {
        r["repo"] = json!(CIG_COMPILE);
    }
    if frames {
        r["backtrace"] = json!(["cigcompile::build::run (src/build.rs:30:9)"]);
    }
    r
}

/// A crash report as `cigar` writes it (origin `cigar-script`).
fn cigar_report(repo: bool, frames: bool) -> Value {
    let mut r = json!({
        "id": "20261111T090100-a1a1a1", "fingerprint": "a1a1a1a1", "version": "0.1.0",
        "os": "linux", "arch": "x86_64", "timestamp": "2026-11-11T09:01:00Z",
        "command": ["cigar", "check", "<path>"], "cwd": "/d1/d2",
        "panic_message": "called `Option::unwrap()` on a `None` value",
        "panic_location": "src/check.rs:88:14",
        "backtrace": [],
        "active_run_id": null, "sent": null, "rolled_back": null,
        "kind": "crash", "origin": "cigar-script",
        "queue": {"requested": false, "attempts": 0},
    });
    if repo {
        r["repo"] = json!(CIGAR);
    }
    if frames {
        r["backtrace"] = json!(["cigarscript::check::run (src/check.rs:88:14)"]);
    }
    r
}

/// Put both tools' reports in the ashtray the way the tools do.
fn plant(fx: &Fixture, repo: bool, frames: bool) {
    std::fs::create_dir_all(ashtray(fx)).unwrap();
    // the home is the person's alone, as the installer insists, whatever the umask made
    #[cfg(unix)]
    for d in [fx.cigscript(), ashtray(fx)] {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    for r in [cig_compile_report(repo, frames), cigar_report(repo, frames)] {
        let id = r["id"].as_str().unwrap().to_string();
        std::fs::write(ashtray(fx).join(format!("{id}.json")), r.to_string()).unwrap();
    }
}

fn api_env(t: &FakeTracker) -> Vec<(&'static str, String)> {
    vec![
        ("CIGI_ROUTES", "api".to_string()),
        ("CIG_GITHUB_TOKEN", TOKEN.to_string()),
        ("CIGI_GITHUB_API", t.url()),
    ]
}

fn with<'a>(v: &'a [(&'static str, String)]) -> Vec<(&'static str, &'a str)> {
    v.iter().map(|(k, x)| (*k, x.as_str())).collect()
}

#[test]
fn a_crash_in_each_tool_is_listed_together_with_its_own_repository() {
    // every shape a tool's report can come in: with its repo, and without (an older build);
    // with its own frames, and without
    for (repo, frames) in [(true, true), (true, false), (false, true), (false, false)] {
        let fx = Fixture::new();
        plant(&fx, repo, frames);
        let l = json_of(&cigi(&fx, &[], &["--json", "crash", "list"]));
        let rows = l["data"]["reports"].as_array().unwrap();
        assert_eq!(rows.len(), 2, "repo={repo} frames={frames}: {l}");
        let row = |origin: &str| {
            rows.iter()
                .find(|r| r["origin"] == origin)
                .unwrap_or_else(|| panic!("a row for {origin}: {l}"))
                .clone()
        };
        assert_eq!(
            row("cig-compile")["target"],
            CIG_COMPILE,
            "repo={repo} frames={frames}"
        );
        assert_eq!(
            row("cigar-script")["target"],
            CIGAR,
            "repo={repo} frames={frames}"
        );
        assert_eq!(row("cig-compile")["state"], "local");
        assert_eq!(l["data"]["unreadable"], 0);
    }
}

#[test]
fn send_files_each_report_in_its_own_repository() {
    let fx = Fixture::new();
    plant(&fx, true, true);
    let t = FakeTracker::start();
    t.require_token(TOKEN);
    let env = api_env(&t);
    for id in ["20261111T090000-c0c0c0", "20261111T090100-a1a1a1"] {
        let o = cigi(&fx, &with(&env), &["--json", "crash", "send", id]);
        assert_eq!(code(&o), 0, "{}", text(&o));
    }
    let mut repos: Vec<String> = t.issues().into_iter().map(|i| i.repo).collect();
    repos.sort();
    assert_eq!(repos, vec![CIG_COMPILE.to_string(), CIGAR.to_string()]);
    let l = json_of(&cigi(&fx, &[], &["--json", "crash", "list"]));
    for r in l["data"]["reports"].as_array().unwrap() {
        assert_eq!(r["state"], "filed", "{r}");
    }
}

#[test]
fn a_report_with_no_repo_and_no_frames_is_routed_by_its_origin() {
    let fx = Fixture::new();
    plant(&fx, false, false);
    let t = FakeTracker::start();
    t.require_token(TOKEN);
    let env = api_env(&t);
    let o = cigi(&fx, &with(&env), &["--json", "crash", "send", "--all"]);
    assert_eq!(code(&o), 0, "{}", text(&o));
    let mut repos: Vec<String> = t.issues().into_iter().map(|i| i.repo).collect();
    repos.sort();
    assert_eq!(repos, vec![CIG_COMPILE.to_string(), CIGAR.to_string()]);
}

#[test]
fn submit_routes_cig_compile_and_cigar_by_origin() {
    // beside `submit_names_its_origin_and_repo` (a6_crash.rs): the two tools' own origin names,
    // and the older spellings, each land on its own repository
    let cases = [
        ("cig-compile", CIG_COMPILE),
        ("cigcompile", CIG_COMPILE),
        ("cigar-script", CIGAR),
        ("cigar", CIGAR),
        ("cigarscript", CIGAR),
    ];
    for (origin, repo) in cases {
        let fx = Fixture::new();
        let f = fx.user_home.join("handed-in.json");
        let body = json!({
            "panic_message": "boom", "panic_location": "src/main.rs:1:1",
            "version": "0.3.0", "kind": "crash",
        });
        std::fs::write(&f, body.to_string()).unwrap();
        let o = cigi(
            &fx,
            &[],
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
        assert_eq!(code(&o), 0, "{origin}: {}", text(&o));
        let id = json_of(&o)["data"]["id"].as_str().unwrap().to_string();
        let s = json_of(&cigi(&fx, &[], &["--json", "crash", "show", &id]));
        assert_eq!(s["data"]["target"], repo, "{origin}");
        assert_eq!(s["data"]["report"]["origin"], origin);
    }
}

#[test]
fn reports_never_keeps_both_tools_reports_at_home() {
    let fx = Fixture::new();
    plant(&fx, true, true);
    // the setting the tools read before they save anything, set the way a person sets it
    let o = cigi(&fx, &[], &["config", "set", "reports", "never"]);
    assert_eq!(code(&o), 0, "{}", text(&o));
    let g = json_of(&cigi(&fx, &[], &["--json", "config", "get", "reports"]));
    assert_eq!(g["data"]["value"], "never", "{g}");
    assert_eq!(g["data"]["source"], "file", "{g}");
    // and nothing leaves: not by name, not with --all
    let t = FakeTracker::start();
    t.require_token(TOKEN);
    let env = api_env(&t);
    for args in [
        vec!["--json", "crash", "send", "20261111T090000-c0c0c0"],
        vec!["--json", "crash", "send", "20261111T090100-a1a1a1"],
        vec!["--json", "crash", "send", "--all"],
    ] {
        let o = cigi(&fx, &with(&env), &args);
        assert_eq!(code(&o), 2, "{args:?}: {}", text(&o));
        assert!(text(&o).contains("E1857"), "{}", text(&o));
    }
    assert!(t.requests().is_empty(), "nothing may leave");
    // the reports stay listed
    let l = json_of(&cigi(&fx, &[], &["--json", "crash", "list"]));
    assert_eq!(l["data"]["reports"].as_array().unwrap().len(), 2);
    assert_eq!(l["data"]["reports_setting"], "never");
}

#[test]
fn the_answer_the_tools_read_is_default_before_anyone_sets_it() {
    // a tool saves a report unless value is never AND the source is not the default: before
    // anything is set the answer says `ask` from the default
    let fx = Fixture::new();
    let g = json_of(&cigi(&fx, &[], &["--json", "config", "get", "reports"]));
    assert_eq!(g["data"]["value"], "ask", "{g}");
    assert_eq!(g["data"]["source"], "default", "{g}");
}
