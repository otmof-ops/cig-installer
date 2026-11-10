// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! The installer's side of the family contract (docs/decisions/0003): `doctor --tools` asks each
//! product for `__doctor --json`, cigo's guard comes down before cigo goes,
//! `cig crash announce-fixed` runs after an
//! update that changed cig, and uninstall clears the products' caches.
//!
//! The products are the fixture's fakes: shell scripts, so these tests are Unix only.
#![cfg(unix)]

mod common;

use std::path::Path;

use cigi_testkit::Fixture;
use common::{cigi, fixture, ok, text};
use serde_json::Value;

const CIG_DOCTOR: &str = r#"{"tool":"cig","version":"1.20.0","findings":[
  {"check":"run records","severity":"ok","found":"12 runs, all readable","because":"every run.json parsed","fix":null},
  {"check":"ashtray","severity":"action","found":"3 reports are stuck in the queue","because":"the queue holds reports older than a week","fix":"cig __doctor --fix"},
  {"check":"box","severity":"note","found":"no containment box on this kernel","because":"the box probe found no user namespaces","fix":null}]}"#;

/// cig 1.20.0 with a self-check; cigo with no `__doctor`; cigcompile with an answer that is JSON
/// and not the findings list; cigar with an empty, clean self-check.
fn setup() -> Fixture {
    let mut fx = fixture();
    fx.product("cig", "1.20.0")
        .doctor_json(CIG_DOCTOR)
        .shell("if [ \"$1\" = \"doctor\" ]; then echo called >> \"$HOME/doctor-called\"; fi");
    fx.product("cigcompile", "0.2.0")
        .doctor_json(r#"{"hello":"world"}"#);
    fx.product("cigar", "0.1.0")
        .doctor_json(r#"{"tool":"cigar","version":"0.1.0","findings":[]}"#);
    fx.carton(
        "0.1.0",
        &[
            ("cig", "1.20.0"),
            ("cigo", "1.17.0"),
            ("cigcompile", "0.2.0"),
            ("cigar", "0.1.0"),
        ],
    );
    fx.publish("stable");
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    fx
}

fn run(fx: &Fixture, env: &[(&str, &str)], args: &[&str]) -> std::process::Output {
    let mut c = fx.cigi();
    c.env("CIGI_ALLOW_SINGLE_SOURCE", "true").arg("--yes");
    for (k, v) in env {
        c.env(k, v);
    }
    c.args(args);
    cigi_testkit::output_retrying(&mut c).expect("run ciginstaller")
}

fn data(o: &std::process::Output) -> Value {
    let line = String::from_utf8_lossy(&o.stdout)
        .lines()
        .last()
        .unwrap_or("")
        .to_string();
    let v: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {}", text(o)));
    v["data"].clone()
}

fn tool<'a>(d: &'a Value, name: &str) -> &'a Value {
    d["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["tool"].as_str().unwrap().starts_with(name))
        .unwrap_or_else(|| panic!("no {name} in {d:#}"))
}

fn write(p: &Path, body: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

// ------------------------------------------------------------------ doctor --tools

#[test]
fn doctor_tools_shows_the_tools_own_findings_and_never_calls_doctor() {
    let fx = setup();
    let o = run(&fx, &[], &["--json", "doctor", "--tools"]);
    let d = data(&o);
    // cig's action counts: the exit code is the installer's own 2
    assert_eq!(o.status.code(), Some(2), "{d:#}");
    let cig = tool(&d, "cig 1.20.0");
    assert_eq!(cig["self_check"], true);
    let f = cig["findings"].as_array().unwrap();
    assert_eq!(f.len(), 3);
    assert_eq!(f[1]["severity"], "action");
    assert_eq!(f[1]["section"], "ashtray");
    assert_eq!(f[1]["fix"], "cig __doctor --fix");
    assert_eq!(f[1]["because"], "the queue holds reports older than a week");
    assert_eq!(f[2]["severity"], "note");
    assert!(d["needs_action"].as_u64().unwrap() >= 1);
    // a clean tool shows no findings and no note
    let cigar = tool(&d, "cigar");
    assert_eq!(cigar["self_check"], true);
    assert!(cigar["findings"].as_array().unwrap().is_empty());
    // the stub `<tool> doctor` is never called: it would come back here
    assert!(
        !fx.user_home.join("doctor-called").exists(),
        "`cig doctor` was called"
    );
}

#[test]
fn a_build_without_a_self_check_gets_a_note() {
    let fx = setup();
    let d = data(&run(&fx, &[], &["--json", "doctor", "--tools"]));
    let cigo = tool(&d, "cigo 1.17.0");
    assert_eq!(cigo["self_check"], false);
    assert_eq!(
        cigo["note"],
        "this cigo build has no self-check; it needs a newer build"
    );
    assert!(cigo["code"].is_null());
}

#[test]
fn an_answer_that_is_not_the_findings_shape_is_e1879() {
    let fx = setup();
    let d = data(&run(&fx, &[], &["--json", "doctor", "--tools"]));
    let c = tool(&d, "cigcompile");
    assert_eq!(c["code"], "E1879");
    assert_eq!(c["ok"], false);
}

#[test]
fn doctor_without_tools_runs_no_tool() {
    let fx = setup();
    let d = data(&run(&fx, &[], &["--json", "doctor"]));
    assert!(d["tools"].as_array().unwrap().is_empty());
}

// ------------------------------------------------------------------ the guard

const GUARD_SHELL: &str = "if [ \"$1 $2 $3\" = \"guard uninstall --yes\" ]; then echo ran >> \"$HOME/guard.log\"; if [ -n \"$FAKE_GUARD_EXIT\" ]; then echo \"guard: refused\" >&2; exit \"$FAKE_GUARD_EXIT\"; fi; rm -rf \"$HOME/.cig-orchestrator\"; exit 0; fi";

fn with_guard() -> Fixture {
    let mut fx = fixture();
    fx.product("cigo", "1.17.0").shell(GUARD_SHELL);
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.17.0")]);
    fx.publish("stable");
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    write(
        &fx.user_home.join(".cig-orchestrator/guard/state.json"),
        "{}\n",
    );
    fx
}

fn guard_log(fx: &Fixture) -> String {
    std::fs::read_to_string(fx.user_home.join("guard.log")).unwrap_or_default()
}

#[test]
fn uninstall_cigo_takes_the_guard_down_first() {
    let fx = with_guard();
    let o = run(&fx, &[], &["--json", "uninstall", "cigo"]);
    ok(&o);
    assert_eq!(guard_log(&fx), "ran\n");
    assert_eq!(data(&o)["guard"], true);
    assert!(!fx.cigscript().join("toolchains/cigo").exists());
}

#[test]
fn uninstall_cigo_without_a_guard_does_not_call_it() {
    let mut fx = fixture();
    fx.product("cigo", "1.17.0").shell(GUARD_SHELL);
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.17.0")]);
    fx.publish("stable");
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    ok(&run(&fx, &[], &["uninstall", "cigo"]));
    assert_eq!(guard_log(&fx), "");
}

#[test]
fn a_failing_guard_uninstall_stops_the_uninstall() {
    let fx = with_guard();
    let o = run(&fx, &[("FAKE_GUARD_EXIT", "3")], &["uninstall", "cigo"]);
    assert_ne!(o.status.code(), Some(0));
    let t = text(&o);
    assert!(t.contains("E1829"), "{t}");
    assert!(t.contains("cigo guard uninstall --yes"), "{t}");
    assert!(t.contains("guard: refused"), "{t}");
    // nothing was removed
    assert!(fx.cigscript().join("toolchains/cigo/1.17.0").exists());
    assert_eq!(guard_log(&fx), "ran\n");
}

#[test]
fn uninstall_all_takes_the_guard_down_and_a_failure_stops_it() {
    let fx = with_guard();
    let o = run(
        &fx,
        &[("FAKE_GUARD_EXIT", "1")],
        &["uninstall", "--all", "--yes-irreversible"],
    );
    assert_ne!(o.status.code(), Some(0));
    assert!(text(&o).contains("E1829"), "{}", text(&o));
    assert!(fx.cigscript().join("installer/state.json").exists());
    ok(&run(
        &fx,
        &[],
        &["uninstall", "--all", "--yes-irreversible"],
    ));
    assert_eq!(guard_log(&fx), "ran\nran\n");
    assert!(!fx.cigscript().join("installer").exists());
}

#[test]
fn a_dry_run_says_the_guard_would_come_down_and_runs_nothing() {
    let fx = with_guard();
    let o = run(&fx, &[], &["--json", "uninstall", "cigo", "--dry-run"]);
    ok(&o);
    assert_eq!(data(&o)["guard"], true);
    assert_eq!(guard_log(&fx), "");
}

// ------------------------------------------------------------------ announce-fixed

const ANNOUNCE: &str = "if [ \"$1 $2\" = \"crash announce-fixed\" ]; then echo called >> \"$HOME/announce.log\"; echo 'a bug you reported (#7) is fixed in 1.21.0'; exit \"${FAKE_ANNOUNCE_EXIT:-0}\"; fi";

fn two_cartons(new_cig_shell: Option<&str>, cig_changes: bool) -> Fixture {
    let mut fx = fixture();
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.17.0")]);
    let new_cig = if cig_changes { "1.21.0" } else { "1.20.0" };
    {
        let p = fx.product("cig", new_cig);
        if let Some(s) = new_cig_shell {
            p.shell(s);
        }
    }
    fx.carton("0.2.0", &[("cig", new_cig), ("cigo", "1.18.0")]);
    fx.publish("stable");
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    fx
}

fn announced(fx: &Fixture) -> String {
    std::fs::read_to_string(fx.user_home.join("announce.log")).unwrap_or_default()
}

#[test]
fn an_update_that_changed_cig_announces_fixed_reports() {
    let fx = two_cartons(Some(ANNOUNCE), true);
    let o = run(&fx, &[], &["update", "--no-trial"]);
    ok(&o);
    assert_eq!(announced(&fx), "called\n");
    assert!(text(&o).contains("a bug you reported (#7) is fixed in 1.21.0"));
}

#[test]
fn a_failing_announce_fixed_does_not_fail_the_update() {
    let fx = two_cartons(Some(ANNOUNCE), true);
    let o = run(
        &fx,
        &[("FAKE_ANNOUNCE_EXIT", "1")],
        &["update", "--no-trial"],
    );
    ok(&o);
    assert_eq!(announced(&fx), "called\n");
    assert!(text(&o).contains("did not finish well"), "{}", text(&o));
}

#[test]
fn a_cig_without_the_subcommand_is_skipped_quietly() {
    let fx = two_cartons(
        Some("if [ \"$1\" = \"crash\" ]; then echo \"error: unrecognized subcommand\" >&2; exit 2; fi"),
        true,
    );
    let o = run(&fx, &[], &["update", "--no-trial"]);
    ok(&o);
    let t = text(&o);
    assert!(!t.contains("announce-fixed"), "{t}");
    assert!(!t.contains("unrecognized"), "{t}");
}

#[test]
fn an_update_that_left_cig_alone_does_not_call_it() {
    let fx = two_cartons(Some(ANNOUNCE), false);
    ok(&run(&fx, &[], &["update", "--no-trial"]));
    assert_eq!(announced(&fx), "");
}

// ------------------------------------------------------------------ product caches

fn make_caches(fx: &Fixture) {
    let h = &fx.user_home;
    write(&h.join(".cache/cigar/run/prog-1"), "built\n");
    write(&h.join(".cache/other/keep"), "not cigar's\n");
    let root = fx.cigscript();
    write(
        &root.join("cigcompile/runtimes/1.20.0/linux-x86_64/cig"),
        "rt\n",
    );
    write(&root.join("cache/compile/index.json"), "{}\n");
}

#[test]
fn uninstall_cigar_removes_its_run_cache_and_lists_it() {
    let fx = setup();
    make_caches(&fx);
    let o = run(&fx, &[], &["--json", "uninstall", "cigar"]);
    ok(&o);
    let cache = fx.user_home.join(".cache/cigar/run");
    assert!(!cache.exists());
    assert!(!fx.user_home.join(".cache/cigar").exists(), "empty parent");
    assert!(fx.user_home.join(".cache/other/keep").exists());
    let d = data(&o);
    let removed = d["removed"].to_string();
    assert!(removed.contains(&cache.display().to_string()), "{removed}");
    // cigcompile's are not cigar's
    assert!(fx.cigscript().join("cigcompile/runtimes").exists());
}

#[test]
fn uninstall_cigcompile_removes_runtimes_and_cache() {
    let fx = setup();
    make_caches(&fx);
    ok(&run(&fx, &[], &["uninstall", "cigcompile"]));
    let root = fx.cigscript();
    assert!(!root.join("cigcompile").exists());
    assert!(!root.join("cache").exists());
    assert!(fx.user_home.join(".cache/cigar/run/prog-1").exists());
}

#[test]
fn uninstall_all_clears_every_product_cache_and_keeps_user_data() {
    let fx = setup();
    make_caches(&fx);
    write(&fx.cigscript().join("runs/r1/run.json"), "{}\n");
    write(&fx.cigscript().join("cache/mine/x"), "user's\n");
    let o = run(
        &fx,
        &[],
        &["--json", "uninstall", "--all", "--yes-irreversible"],
    );
    ok(&o);
    assert!(!fx.user_home.join(".cache/cigar").exists());
    let root = fx.cigscript();
    assert!(!root.join("cigcompile").exists());
    assert!(!root.join("cache/compile").exists());
    assert!(root.join("runs/r1/run.json").exists());
    assert!(root.join("cache/mine/x").exists());
    assert!(fx.user_home.join(".cache/other/keep").exists());
    let removed = data(&o)["removed"].to_string();
    assert!(removed.contains("cigar's run cache"), "{removed}");
    assert!(
        removed.contains("cigcompile's fetched runtimes"),
        "{removed}"
    );
}

// ------------------------------------------------------------------ __doctor details

/// cig's `__doctor --json` exits 0 whatever it finds, and another tool may not: the JSON is read,
/// not the exit code. A test may set CIG_INTERNAL_NO_NS_PROBE=1, which reaches the child.
#[test]
fn the_json_is_read_not_the_exit_code_and_the_environment_reaches_the_tool() {
    let mut fx = fixture();
    fx.product("cig", "1.20.0").shell(
        "if [ \"$1\" = \"__doctor\" ]; then echo \"$CIG_INTERNAL_NO_NS_PROBE\" > \"$HOME/probe\"; echo '{\"tool\":\"cig\",\"version\":\"1.20.0\",\"findings\":[{\"check\":\"box\",\"severity\":\"action\",\"found\":\"x\",\"because\":\"y\",\"fix\":\"z\"}]}'; exit 3; fi",
    );
    fx.carton("0.1.0", &[("cig", "1.20.0")]);
    fx.publish("stable");
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    let o = run(
        &fx,
        &[("CIG_INTERNAL_NO_NS_PROBE", "1")],
        &["--json", "doctor", "--tools"],
    );
    let d = data(&o);
    let cig = tool(&d, "cig 1.20.0");
    assert_eq!(cig["self_check"], true, "{d:#}");
    assert_eq!(cig["findings"][0]["fix"], "z");
    assert_eq!(
        std::fs::read_to_string(fx.user_home.join("probe")).unwrap(),
        "1\n"
    );
}
