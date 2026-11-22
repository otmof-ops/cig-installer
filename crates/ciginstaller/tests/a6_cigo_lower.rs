// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `cigo __lower --json` after a committed transaction that changed what cigo's launcher sees of
//! the cig and the cigo it starts (cig-orchestrator#142). The transaction has already finished, so
//! the step never changes its result: a cigo from before the command is silent, a crash, a timeout
//! or an answer that is not cigo's document is one line and nothing in the exit code.
//!
//! The products are the fixture's fakes, so these tests are Unix only.
#![cfg(unix)]

mod common;

use std::path::{Path, PathBuf};
use std::process::Output;

use cigi_testkit::Fixture;
use common::{cigi, fixture, ok, text};
use serde_json::Value;

/// Lines for the fake cigo: every `__lower` call is recorded as one line in `$HOME/lower.log`
/// (its arguments, the folder it ran in, and how many entries that folder held), and answered
/// with `answer`, as cigo's `__lower --json` answers.
fn answers(answer: &str) -> String {
    format!(
        r#"if [ "$1" = "__lower" ]; then
  n=$(($(ls -A . 2>/dev/null | wc -l)))
  printf '%s|cwd=%s|entries=%s\n' "$*" "$PWD" "$n" >> "$HOME/lower.log"
  printf '%s\n' '{answer}'
  exit 0
fi"#
    )
}

/// cigo lowered its script on this call and kept the module.
const LOWERED: &str = r#"{"tool":"cigo","version":"1.17.0","cig":"1.25.0","engine":"vm","kept":true,"lowered":true,"reason":null}"#;
/// The module was already kept: nothing was lowered.
const KEPT: &str = r#"{"tool":"cigo","version":"1.17.0","cig":"1.25.0","engine":"vm","kept":true,"lowered":false,"reason":null}"#;
/// cigo stays on the tree-walker, and says why.
const TREE: &str = r#"{"tool":"cigo","version":"1.17.0","cig":"1.24.0","engine":"tree","kept":false,"lowered":false,"reason":"cig 1.24.0 runs scripts on the tree-walker only"}"#;

/// A cigo from before `__lower`: it does not know the command.
const UNKNOWN: &str = r#"if [ "$1" = "__lower" ]; then
  echo 'error: unknown command `__lower`' >&2
  exit 2
fi"#;

/// An answer that is not cigo's document, with an escape sequence in it.
const NONSENSE: &str = r#"if [ "$1" = "__lower" ]; then
  printf 'not the document \033[2J at all\n'
  exit 0
fi"#;

/// A cigo that does not come back within the limit a test sets.
const SLEEPS: &str = r#"if [ "$1" = "__lower" ]; then
  echo started > "$HOME/lower.started"
  sleep 20
  exit 0
fi"#;

fn run(fx: &Fixture, env: &[(&str, &str)], args: &[&str]) -> Output {
    let mut c = fx.cigi();
    c.env("CIGI_ALLOW_SINGLE_SOURCE", "true").arg("--yes");
    for (k, v) in env {
        c.env(k, v);
    }
    c.args(args);
    cigi_testkit::output_retrying(&mut c).expect("run ciginstaller")
}

/// The one JSON document of a `--json` run.
fn doc(o: &Output) -> Value {
    let line = String::from_utf8_lossy(&o.stdout)
        .lines()
        .last()
        .unwrap_or("")
        .to_string();
    serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {}", text(o)))
}

/// The report's detail lines that are about cigo's lowering.
fn said(d: &Value) -> Vec<String> {
    d["detail"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .filter(|l| l.contains("lower") || l.contains("cig's VM"))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn calls(fx: &Fixture) -> Vec<String> {
    std::fs::read_to_string(fx.user_home.join("lower.log"))
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

fn forget_calls(fx: &Fixture) {
    let _ = std::fs::remove_file(fx.user_home.join("lower.log"));
}

/// The folder a recorded call ran in.
fn cwd_of(line: &str) -> PathBuf {
    PathBuf::from(
        line.split('|')
            .find_map(|p| p.strip_prefix("cwd="))
            .unwrap_or_else(|| panic!("no cwd in {line}")),
    )
}

/// One published carton, 0.1.0, with cig, cigo and cigcompile; cigo runs `cigo_lines` first.
fn setup(cigo_lines: &str) -> Fixture {
    let mut fx = fixture();
    fx.product("cigo", "1.17.0").shell(cigo_lines);
    fx.product("cig", "1.20.0");
    fx.product("cigcompile", "0.2.0");
    fx.carton(
        "0.1.0",
        &[
            ("cig", "1.20.0"),
            ("cigo", "1.17.0"),
            ("cigcompile", "0.2.0"),
        ],
    );
    fx.publish("stable");
    fx
}

/// A second carton, 0.2.0, published beside the first.
fn second_carton(fx: &mut Fixture, cig: &str, cigcompile: &str) {
    if cig != "1.20.0" {
        fx.product("cig", cig);
    }
    if cigcompile != "0.2.0" {
        fx.product("cigcompile", cigcompile);
    }
    fx.carton(
        "0.2.0",
        &[("cig", cig), ("cigo", "1.17.0"), ("cigcompile", cigcompile)],
    );
    fx.publish("stable");
}

const INSTALL: [&str; 5] = ["--json", "install", "carton", "--carton", "0.1.0"];

// ------------------------------------------------------------------ when it runs

#[test]
fn an_install_runs_it_once_in_an_empty_folder_outside_the_home_and_says_what_cigo_did() {
    let fx = setup(&answers(LOWERED));
    let o = run(&fx, &[], &INSTALL);
    ok(&o);
    let calls = calls(&fx);
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert!(calls[0].starts_with("__lower --json|"), "{}", calls[0]);
    assert!(calls[0].ends_with("|entries=0"), "{}", calls[0]);
    let cwd = cwd_of(&calls[0]);
    let real = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    for home in [real(&fx.user_home), real(&fx.cigscript())] {
        assert!(
            !cwd.starts_with(&home),
            "{} is inside {}",
            cwd.display(),
            home.display()
        );
    }
    // the folder was the step's alone, and is gone
    assert!(!cwd.exists(), "{} is still there", cwd.display());
    let d = doc(&o);
    let said = said(&d);
    assert_eq!(said.len(), 1, "{d:#}");
    assert!(said[0].contains("lowered for cig's VM"), "{}", said[0]);
    assert_eq!(d["data"]["cigo_lower"]["engine"], "vm");
    assert_eq!(d["data"]["cigo_lower"]["lowered"], true);
}

#[test]
fn an_update_of_cig_alone_runs_it_once() {
    let mut fx = setup(&answers(LOWERED));
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    forget_calls(&fx);
    second_carton(&mut fx, "1.21.0", "0.2.0");
    let o = run(&fx, &[], &["--json", "update", "--no-trial"]);
    ok(&o);
    assert_eq!(calls(&fx).len(), 1, "{:?}", calls(&fx));
    assert_eq!(doc(&o)["data"]["cigo_lower"]["engine"], "vm");
}

#[test]
fn an_update_of_another_tool_runs_it_too_and_says_nothing_when_the_module_was_kept() {
    // the shim index is written again by every update, and cigo's launcher reads its size and
    // time: cigo is asked, finds its module still kept, and there is nothing to tell the person
    let mut fx = setup(&answers(KEPT));
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    forget_calls(&fx);
    second_carton(&mut fx, "1.20.0", "0.3.0");
    let o = run(&fx, &[], &["--json", "update", "--no-trial"]);
    ok(&o);
    assert_eq!(calls(&fx).len(), 1, "{:?}", calls(&fx));
    let d = doc(&o);
    assert!(said(&d).is_empty(), "{d:#}");
    assert_eq!(d["data"]["cigo_lower"]["kept"], true);
    assert_eq!(d["data"]["cigo_lower"]["lowered"], false);
}

#[test]
fn a_rollback_that_undoes_an_update_runs_it_once() {
    let mut fx = setup(&answers(LOWERED));
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    second_carton(&mut fx, "1.21.0", "0.2.0");
    ok(&run(&fx, &[], &["update", "--no-trial"]));
    forget_calls(&fx);
    let o = run(&fx, &[], &["--json", "rollback"]);
    ok(&o);
    assert_eq!(calls(&fx).len(), 1, "{:?}", calls(&fx));
    assert_eq!(doc(&o)["data"]["cigo_lower"]["engine"], "vm");
}

#[test]
fn a_switch_to_a_carton_with_another_cig_runs_it_once() {
    let mut fx = setup(&answers(LOWERED));
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    second_carton(&mut fx, "1.21.0", "0.2.0");
    ok(&run(&fx, &[], &["install", "carton", "--carton", "0.2.0"]));
    let machine = |fx: &Fixture| -> String {
        let s = std::fs::read_to_string(fx.cigscript().join("installer/state.json")).unwrap();
        let v: Value = serde_json::from_str(&s).unwrap();
        v["machine_carton"].as_str().unwrap_or("").to_string()
    };
    // whichever of the two the machine runs now, switch to the other
    let other = if machine(&fx) == "0.1.0" {
        "0.2.0"
    } else {
        "0.1.0"
    };
    forget_calls(&fx);
    let o = run(&fx, &[], &["--json", "use", other]);
    ok(&o);
    assert_eq!(machine(&fx), other);
    assert_eq!(calls(&fx).len(), 1, "{:?}", calls(&fx));
    assert_eq!(doc(&o)["data"]["cigo_lower"]["engine"], "vm");
}

#[test]
fn a_repair_that_links_a_shim_again_runs_it_once_and_one_with_nothing_to_do_does_not() {
    let fx = setup(&answers(LOWERED));
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    forget_calls(&fx);
    ok(&run(&fx, &[], &["repair"]));
    assert!(calls(&fx).is_empty(), "{:?}", calls(&fx));
    std::fs::remove_file(fx.cigscript().join("bin").join("cig")).unwrap();
    let o = run(&fx, &[], &["--json", "repair"]);
    ok(&o);
    assert_eq!(calls(&fx).len(), 1, "{:?}", calls(&fx));
    assert_eq!(doc(&o)["data"]["cigo_lower"]["engine"], "vm");
}

#[test]
fn a_repair_of_another_tool_runs_it_only_if_it_wrote_what_the_launcher_reads() {
    let fx = setup(&answers(LOWERED));
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    forget_calls(&fx);
    let stray = fx
        .cigscript()
        .join("toolchains")
        .join("cigcompile/0.2.0/bin/stray");
    std::fs::write(&stray, "extra").unwrap();
    let index = fx.cigscript().join("installer/shim-index");
    let stamp = |p: &Path| {
        let m = std::fs::metadata(p).unwrap();
        (m.len(), m.modified().unwrap())
    };
    let before = stamp(&index);
    let o = run(&fx, &[], &["--json", "repair", "--tool", "cigcompile"]);
    ok(&o);
    assert!(!stray.exists(), "the repair removed the stray file");
    if stamp(&index) == before {
        assert!(calls(&fx).is_empty(), "{:?}", calls(&fx));
        assert!(doc(&o)["data"]["cigo_lower"].is_null());
    } else {
        // the repair wrote the index again, which cigo's launcher reads: cigo is asked
        assert_eq!(calls(&fx).len(), 1, "{:?}", calls(&fx));
    }
}

#[test]
fn shims_sync_runs_it_when_it_links_a_shim_again() {
    let fx = setup(&answers(LOWERED));
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    forget_calls(&fx);
    std::fs::remove_file(fx.cigscript().join("bin").join("cig")).unwrap();
    let o = run(&fx, &[], &["--json", "shims", "sync"]);
    ok(&o);
    assert_eq!(calls(&fx).len(), 1, "{:?}", calls(&fx));
    assert_eq!(doc(&o)["data"]["cigo_lower"]["engine"], "vm");
}

#[test]
fn a_self_update_runs_it_once_after_the_shims_follow_the_new_build() {
    let mut fx = fixture();
    fx.product("cigo", "1.17.0").shell(&answers(LOWERED));
    fx.product("cig", "1.20.0");
    fx.product("ciginstaller", "0.1.1");
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.17.0")]);
    fx.carton(
        "0.1.1",
        &[
            ("cig", "1.20.0"),
            ("cigo", "1.17.0"),
            ("ciginstaller", "0.1.1"),
        ],
    );
    fx.publish("stable");
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    let shim_log = fx.user_home.join("shim.log");
    // the installer the home holds does the update, as a person's would
    let managed = |args: &[&str]| -> Output {
        let base = fx.cigi();
        let mut c = std::process::Command::new(fx.cigscript().join("bin").join("ciginstaller"));
        c.env_clear();
        for (k, v) in base.get_envs() {
            if let Some(v) = v {
                c.env(k, v);
            }
        }
        c.env("CIGI_ALLOW_SINGLE_SOURCE", "true")
            .env("CIGI_FAKE_SHIM_LOG", &shim_log)
            .current_dir(&fx.project)
            .args(args);
        cigi_testkit::output_retrying(&mut c).expect("run the installed ciginstaller")
    };
    ok(&managed(&["--yes", "list"]));
    let o = managed(&["--yes", "--json", "self", "update"]);
    ok(&o);
    // the fixture's new shim is a stand-in that starts no tool: it records the name it was started
    // under and its arguments, and answers nothing, which the installer takes without a word
    let seen = std::fs::read_to_string(&shim_log).unwrap_or_default();
    let started: Vec<&str> = seen.lines().filter(|l| l.starts_with("cigo ")).collect();
    assert_eq!(started, ["cigo __lower --json"], "{seen}");
    let d = doc(&o);
    assert!(d["data"]["cigo_lower"].is_null(), "{d:#}");
    assert!(said(&d).is_empty(), "{d:#}");
}

// ------------------------------------------------------------------ when it does not

#[test]
fn a_dry_run_and_a_check_do_not_run_it() {
    let mut fx = setup(&answers(LOWERED));
    ok(&run(
        &fx,
        &[],
        &["install", "carton", "--carton", "0.1.0", "--dry-run"],
    ));
    assert!(calls(&fx).is_empty(), "{:?}", calls(&fx));
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    forget_calls(&fx);
    second_carton(&mut fx, "1.21.0", "0.2.0");
    ok(&run(&fx, &[], &["update", "--check"]));
    assert!(calls(&fx).is_empty(), "{:?}", calls(&fx));
}

#[test]
fn a_machine_with_no_cigo_runs_nothing() {
    let mut fx = fixture();
    fx.product("cig", "1.20.0");
    fx.carton("0.1.0", &[("cig", "1.20.0")]);
    fx.publish("stable");
    let o = run(&fx, &[], &INSTALL);
    ok(&o);
    let d = doc(&o);
    assert!(d["data"]["cigo_lower"].is_null(), "{d:#}");
    assert!(said(&d).is_empty(), "{d:#}");
}

// ------------------------------------------------------------------ it never changes the result

#[test]
fn a_cigo_from_before_the_command_is_silent() {
    let fx = setup(UNKNOWN);
    let o = run(&fx, &[], &INSTALL);
    ok(&o);
    let d = doc(&o);
    assert!(said(&d).is_empty(), "{d:#}");
    assert!(d["data"]["cigo_lower"].is_null(), "{d:#}");
}

#[test]
fn a_cigo_that_stays_on_the_tree_walker_is_in_the_document_and_not_in_the_report() {
    let fx = setup(&answers(TREE));
    let o = run(&fx, &[], &INSTALL);
    ok(&o);
    let d = doc(&o);
    assert!(said(&d).is_empty(), "{d:#}");
    assert_eq!(d["data"]["cigo_lower"]["engine"], "tree");
    assert_eq!(
        d["data"]["cigo_lower"]["reason"],
        "cig 1.24.0 runs scripts on the tree-walker only"
    );
}

#[test]
fn an_answer_that_is_not_the_document_is_one_line_without_control_characters() {
    let fx = setup(NONSENSE);
    let o = run(&fx, &[], &INSTALL);
    ok(&o);
    let d = doc(&o);
    assert!(d["data"]["cigo_lower"].is_null(), "{d:#}");
    let said = said(&d);
    assert_eq!(said.len(), 1, "{d:#}");
    assert!(said[0].contains("cannot read"), "{}", said[0]);
    assert!(said[0].contains("not the document"), "{}", said[0]);
    assert!(!said[0].chars().any(char::is_control), "{:?}", said[0]);
}

#[test]
fn a_cigo_that_does_not_come_back_is_stopped_at_the_limit_and_the_install_stands() {
    let fx = setup(SLEEPS);
    let o = run(&fx, &[("CIGI_LOWER_TIMEOUT_SECS", "1")], &INSTALL);
    ok(&o);
    assert!(
        fx.user_home.join("lower.started").exists(),
        "the step was started"
    );
    let d = doc(&o);
    assert!(d["data"]["cigo_lower"].is_null(), "{d:#}");
    let said = said(&d);
    assert_eq!(said.len(), 1, "{d:#}");
    assert!(said[0].contains("__lower"), "{}", said[0]);
    // the install is what it would have been
    let state = std::fs::read_to_string(fx.cigscript().join("installer/state.json")).unwrap();
    let state: Value = serde_json::from_str(&state).unwrap();
    assert_eq!(state["machine_carton"], "0.1.0", "{state:#}");
}
