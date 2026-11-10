// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! #22, what ciginstaller gives CIG.OS: the install questions as data (`install --dry-run --json`),
//! answers handed in up front (`--answers`, `--preset`, `--reports`) so a welcome screen can ask
//! them itself, questions asked once and recorded (`status --json` says so), and the image
//! build's preseed: `install --yes carton --from <bundle>` with the answers, no network, no
//! question, no request (#19).
#![cfg(unix)]

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};

use cigi_testkit::{Fixture, Snapshot};
use common::{cigi, fixture, ok, text, type_at_prompts};
use serde_json::{json, Value};

const MIRROR: (&str, &str) = ("CIGI_ALLOW_SINGLE_SOURCE", "true");

fn host() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

fn published() -> Fixture {
    let mut fx = fixture();
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.16.0")]);
    fx.carton("0.2.0", &[("cig", "1.21.0"), ("cigo", "1.17.0")]);
    fx.publish("stable");
    fx
}

fn json_of(o: &Output) -> Value {
    serde_json::from_slice(&o.stdout).unwrap_or_else(|e| panic!("{e}: {}", text(o)))
}

fn data(o: &Output) -> Value {
    ok(o);
    json_of(o)["data"].clone()
}

fn write_answers(dir: &Path, body: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let p = dir.join(format!("answers-{}.json", N.fetch_add(1, Ordering::SeqCst)));
    std::fs::write(&p, body).expect("write answers");
    p
}

/// The installer with no terminal and no `--yes`, as a welcome screen's own process runs it.
fn bare(fx: &Fixture, args: &[&str]) -> Output {
    fx.cigi()
        .env(MIRROR.0, MIRROR.1)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("run ciginstaller")
}

/// The installer on a terminal through util-linux `script`, typing `keys` at its questions.
fn on_tty(fx: &Fixture, args: &str, keys: &[&str]) -> (i32, String) {
    let c = fx.cigi();
    let exe = c.get_program().to_string_lossy().to_string();
    let q = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
    let line = format!("umask 022; {} {args}", q(&exe));
    let mut s = Command::new("script");
    s.env_clear();
    for (k, v) in c.get_envs() {
        if let Some(v) = v {
            s.env(k, v);
        }
    }
    s.env(MIRROR.0, MIRROR.1)
        .args(cigi_testkit::script_args(&line))
        .current_dir(&fx.project);
    type_at_prompts(s, keys)
}

fn setting(fx: &Fixture, key: &str) -> String {
    let d = data(&cigi(fx, &["--json", "config", "get", key]));
    d["value"].as_str().unwrap_or("").to_string()
}

fn first_run(fx: &Fixture) -> Value {
    data(&bare(fx, &["--json", "status"]))["first_run"].clone()
}

fn everything(fx: &Fixture) -> Snapshot {
    Snapshot::take(&fx.cigscript())
}

// ---- 1. the questions as data --------------------------------------------------------------

#[test]
fn the_dry_run_lists_the_questions_and_writes_nothing() {
    let fx = published();
    let before = everything(&fx);
    // the mirror is nowhere: listing the questions fetches nothing
    let o = fx
        .cigi_with_mirror("file:///nowhere-at-all")
        .args(["--json", "install", "--dry-run"])
        .stdin(Stdio::null())
        .output()
        .expect("run");
    let d = data(&o);
    assert_eq!(d["dry_run"], true);
    let qs = d["questions"].as_array().expect("questions");
    let ids: Vec<&str> = qs.iter().map(|q| q["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["what", "preset", "reports"]);
    for q in qs {
        assert!(!q["text"].as_str().unwrap().is_empty(), "{q}");
        assert!(!q["plain"].as_str().unwrap().is_empty(), "{q}");
        assert!(q["text"] != q["plain"], "voice and plain differ: {q}");
        let values: Vec<&str> = q["choices"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["value"].as_str().unwrap())
            .collect();
        assert!(values.contains(&q["default"].as_str().unwrap()), "{q}");
    }
    let preset = &qs[1];
    assert_eq!(preset["writes"], "preset");
    assert_eq!(preset["default"], "balanced");
    let values: Vec<&str> = preset["choices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["value"].as_str().unwrap())
        .collect();
    assert_eq!(values, ["chill", "balanced", "strict", "paranoid"]);
    assert_eq!(qs[2]["writes"], "reports");
    assert_eq!(qs[2]["default"], "ask");
    assert_eq!(qs[0]["writes"], Value::Null);
    assert!(qs.iter().all(|q| q["will_ask"] == true), "fresh machine");
    assert_eq!(d["first_run"]["done"], false);
    assert_eq!(d["first_run"]["pending"], json!(["preset", "reports"]));
    assert_eq!(before.diff(&everything(&fx)), Vec::<String>::new());

    // answers handed in show as given, and are still not written
    let f = write_answers(fx.tmp.path(), r#"{"preset":"strict"}"#);
    let d = data(&bare(
        &fx,
        &[
            "--json",
            "install",
            "--dry-run",
            "--answers",
            &f.to_string_lossy(),
        ],
    ));
    assert_eq!(d["questions"][1]["status"], "given");
    assert_eq!(d["questions"][1]["answer"], "strict");
    assert_eq!(d["questions"][1]["will_ask"], false);
    assert_eq!(d["questions"][2]["will_ask"], true);
    assert_eq!(before.diff(&everything(&fx)), Vec::<String>::new());

    // with a target the plan is as before, and the questions ride along
    let d = data(&bare(
        &fx,
        &["--json", "install", "carton", "--dry-run", "--no-path"],
    ));
    assert_eq!(d["dry_run"], true);
    assert!(d["tools"].is_array() && d["questions"].is_array(), "{d}");
    assert_eq!(d["questions"][0]["will_ask"], false);
}

// ---- 2. answers handed in ------------------------------------------------------------------

#[test]
fn answers_handed_in_set_the_settings_and_nothing_is_asked() {
    let fx = published();
    let f = write_answers(fx.tmp.path(), r#"{"preset":"strict","reports":"never"}"#);
    // on a terminal, so a question would be drawn; none is
    let (code, t) = on_tty(
        &fx,
        &format!("install carton --no-path --answers '{}'", f.display()),
        &["y"],
    );
    assert_eq!(code, 0, "{t}");
    // the one question drawn is the licenses', which is not a first-run question
    assert_eq!(
        t.matches("(enter =").count(),
        1,
        "a question was asked: {t}"
    );
    assert!(!t.contains("send a crash report"), "{t}");
    assert!(!t.contains("how careful should the guard be?"), "{t}");
    assert_eq!(setting(&fx, "preset"), "strict");
    assert_eq!(setting(&fx, "reports"), "never");
    let fr = first_run(&fx);
    assert_eq!(fr["done"], true, "{fr}");
    assert_eq!(fr["pending"], json!([]));
    assert_eq!(
        fr["answers"],
        json!({"preset": "strict", "reports": "never"})
    );

    // flags say the same thing as the file
    let fx2 = published();
    let d = data(&cigi(
        &fx2,
        &[
            "--json",
            "install",
            "carton",
            "--no-path",
            "--preset",
            "paranoid",
            "--reports",
            "always",
        ],
    ));
    assert_eq!(d["first_run"]["done"], true);
    assert_eq!(setting(&fx2, "preset"), "paranoid");
    assert_eq!(setting(&fx2, "reports"), "always");
}

#[test]
fn a_machine_that_is_already_installed_takes_answers_without_a_download() {
    let fx = published();
    ok(&cigi(&fx, &["install", "carton", "--no-path"]));
    // installed with no answers: the questions are still open, for a welcome screen to ask
    let fr = first_run(&fx);
    assert_eq!(fr["done"], false, "{fr}");
    assert_eq!(fr["pending"], json!(["preset", "reports"]));
    let f = write_answers(fx.tmp.path(), r#"{"preset":"chill","reports":"ask"}"#);
    let o = cigi(
        &fx,
        &[
            "--json",
            "install",
            "carton",
            "--no-path",
            "--answers",
            &f.to_string_lossy(),
        ],
    );
    let d = data(&o);
    assert_eq!(d["changed"], false, "nothing was installed: {d}");
    assert_eq!(d["first_run"]["done"], true, "{d}");
    assert_eq!(setting(&fx, "preset"), "chill");
    assert_eq!(first_run(&fx)["done"], true);
}

// ---- 3. a bad answer refuses before anything changes ---------------------------------------

#[test]
fn a_bad_answer_is_a_usage_error_before_anything_changes() {
    let fx = published();
    let before = everything(&fx);
    let dir = fx.tmp.path();
    let long = format!("{{\"preset\":\"{}\"}}", "x".repeat(70 * 1024));
    let cases: Vec<(&str, Vec<String>)> = vec![
        (
            "not a choice",
            vec![
                "--answers".into(),
                write_answers(dir, r#"{"preset":"loud"}"#)
                    .display()
                    .to_string(),
            ],
        ),
        (
            "not a question",
            vec![
                "--answers".into(),
                write_answers(dir, r#"{"colour":"red"}"#)
                    .display()
                    .to_string(),
            ],
        ),
        (
            "what is the subcommand",
            vec![
                "--answers".into(),
                write_answers(dir, r#"{"what":"carton"}"#)
                    .display()
                    .to_string(),
            ],
        ),
        (
            "not a string",
            vec![
                "--answers".into(),
                write_answers(dir, r#"{"preset":3}"#).display().to_string(),
            ],
        ),
        (
            "not an object",
            vec![
                "--answers".into(),
                write_answers(dir, r#"["strict"]"#).display().to_string(),
            ],
        ),
        (
            "not JSON",
            vec![
                "--answers".into(),
                write_answers(dir, "preset = strict").display().to_string(),
            ],
        ),
        (
            "too large",
            vec![
                "--answers".into(),
                write_answers(dir, &long).display().to_string(),
            ],
        ),
        (
            "missing file",
            vec![
                "--answers".into(),
                dir.join("nope.json").display().to_string(),
            ],
        ),
        (
            "flag, not a choice",
            vec!["--reports".into(), "sometimes".into()],
        ),
        (
            "preset flag, not a choice",
            vec!["--preset".into(), "loud".into()],
        ),
        (
            "flag and file disagree",
            vec![
                "--answers".into(),
                write_answers(dir, r#"{"preset":"strict"}"#)
                    .display()
                    .to_string(),
                "--preset".into(),
                "chill".into(),
            ],
        ),
    ];
    for (name, extra) in cases {
        for dry in [false, true] {
            let mut args = vec!["--json", "install", "carton", "--no-path"];
            if dry {
                args = vec!["--json", "install", "--dry-run"];
            }
            let extra_refs: Vec<&str> = extra.iter().map(String::as_str).collect();
            args.extend(extra_refs);
            let o = bare(&fx, &args);
            assert_eq!(o.status.code(), Some(3), "{name} (dry {dry}): {}", text(&o));
            assert!(text(&o).contains("E1870"), "{name}: {}", text(&o));
            assert_eq!(
                before.diff(&everything(&fx)),
                Vec::<String>::new(),
                "{name} changed something"
            );
        }
    }
    // a refusal names the choices
    let o = bare(
        &fx,
        &["--json", "install", "carton", "--reports", "sometimes"],
    );
    assert!(text(&o).contains("always, never, ask"), "{}", text(&o));
}

// ---- 4. asked once -------------------------------------------------------------------------

#[test]
fn a_second_install_does_not_ask_again() {
    let fx = published();
    // the person answers: preset strict (s), reports never (n)
    let (code, t) = on_tty(
        &fx,
        "install carton --carton 0.1.0 --no-path",
        &["s", "n", "y"],
    );
    assert_eq!(code, 0, "{t}");
    assert!(t.contains("how careful should the guard be?"), "{t}");
    assert!(t.contains("send a crash report"), "{t}");
    assert_eq!(setting(&fx, "preset"), "strict");
    assert_eq!(setting(&fx, "reports"), "never");
    let fr = first_run(&fx);
    assert_eq!(fr["done"], true, "{fr}");

    // a later install, same machine, nobody types anything
    let (code, t) = on_tty(&fx, "install carton --carton 0.2.0 --no-path", &[]);
    assert_eq!(code, 0, "{t}");
    assert!(!t.contains("(enter ="), "asked again: {t}");
    assert_eq!(setting(&fx, "preset"), "strict");
    // and the dry run says nothing is left to ask
    let d = data(&bare(&fx, &["--json", "install", "--dry-run"]));
    assert!(
        d["questions"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|q| q["once"] == true)
            .all(|q| q["will_ask"] == false && q["status"] == "answered"),
        "{d}"
    );
}

#[test]
fn what_is_left_open_is_asked_and_only_that() {
    let fx = published();
    // the image answered the preset only
    ok(&cigi(
        &fx,
        &[
            "install",
            "carton",
            "--carton",
            "0.1.0",
            "--no-path",
            "--preset",
            "strict",
        ],
    ));
    let fr = first_run(&fx);
    assert_eq!(fr["done"], false, "{fr}");
    assert_eq!(fr["pending"], json!(["reports"]));
    let (code, t) = on_tty(&fx, "install carton --carton 0.2.0 --no-path", &["n"]);
    assert_eq!(code, 0, "{t}");
    assert!(t.contains("send a crash report"), "{t}");
    assert!(!t.contains("how careful should the guard be?"), "{t}");
    assert_eq!(setting(&fx, "reports"), "never");
    assert_eq!(first_run(&fx)["done"], true);
}

// ---- 5. the preseed, and no request (#19) --------------------------------------------------

struct Counter {
    base: String,
    seen: Arc<Mutex<u32>>,
}

impl Counter {
    fn start() -> Counter {
        let server = tiny_http::Server::http("127.0.0.1:0").expect("bind a counting server");
        let port = server.server_addr().to_ip().expect("ip").port();
        let seen = Arc::new(Mutex::new(0u32));
        let log = Arc::clone(&seen);
        std::thread::spawn(move || {
            for req in server.incoming_requests() {
                *log.lock().unwrap() += 1;
                let _ = req.respond(tiny_http::Response::from_string("{}").with_status_code(404));
            }
        });
        Counter {
            base: format!("http://127.0.0.1:{port}"),
            seen,
        }
    }
    fn count(&self) -> u32 {
        *self.seen.lock().unwrap()
    }
}

#[test]
fn the_image_build_preseed_from_a_bundle_needs_no_network_and_asks_nothing() {
    let fx = published();
    let tar = fx.tmp.path().join("carton-0.1.0.tar");
    ok(&cigi(
        &fx,
        &[
            "bundle",
            "--carton",
            "0.1.0",
            "--targets",
            &host(),
            "--out",
            &tar.to_string_lossy(),
        ],
    ));
    let f = write_answers(fx.tmp.path(), r#"{"preset":"strict","reports":"never"}"#);

    // the image: its mirror and its report route are loopback servers that count every request
    let net = Counter::start();
    let route = Counter::start();
    let image = fixture();
    let o = image
        .cigi_with_mirror(&net.base)
        .env(MIRROR.0, MIRROR.1)
        .env("CIGI_REPORT_ROUTE", format!("{}/reports", route.base))
        .stdin(Stdio::null())
        .args([
            "--yes",
            "--json",
            "install",
            "carton",
            "--no-path",
            "--from",
            &tar.to_string_lossy(),
            "--answers",
            &f.to_string_lossy(),
        ])
        .output()
        .expect("run ciginstaller");
    let d = data(&o);
    assert_eq!(d["carton"], "0.1.0", "{d}");
    assert!(
        d["source"].as_str().unwrap().starts_with("bundle:"),
        "installed from the bundle: {d}"
    );
    assert_eq!(net.count(), 0, "the install made a request");
    assert_eq!(route.count(), 0, "no request without a yes (#19)");
    assert!(!text(&o).contains("(enter ="), "{}", text(&o));

    // the first boot finds the questions answered, and the tools in place
    assert_eq!(first_run(&image)["done"], true);
    assert_eq!(setting(&image, "preset"), "strict");
    assert_eq!(setting(&image, "reports"), "never");
    ok(&image.run_shim("cigo", &image.project, &[]));
    let d = data(
        &image
            .cigi_with_mirror(&net.base)
            .env(MIRROR.0, MIRROR.1)
            .stdin(Stdio::null())
            .args(["--json", "install", "--dry-run"])
            .output()
            .expect("run"),
    );
    assert!(
        d["questions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|q| q["will_ask"] == false || q["id"] == "what"),
        "a welcome screen has nothing left to ask: {d}"
    );
    assert_eq!(net.count(), 0);
    assert_eq!(route.count(), 0);
}
