// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `cigo __lower --json` after a committed transaction that changed which build of cig or cigo
//! the shims start (cig-orchestrator#142). The transaction has already finished, so this step
//! never fails it: an older cigo without the subcommand is silent, a crash or a timeout is one
//! line and nothing in the exit code. The fixture's cigo is a shell script that records how it
//! was called.
//!
//! The products are the fixture's fakes, so these tests are Unix only.
#![cfg(unix)]

mod common;

use std::path::Path;
use std::process::Output;

use cigi_testkit::Fixture;
use common::{cigi, fixture, ok, text};
use serde_json::Value;

/// A cigo shell hook that records every `__lower` call as one line in `$HOME/lower.log`
/// (args, cwd, and whether the cwd was empty when it was called) and answers as cigo's `__lower
/// --json` does.
const LOWER_OK: &str = r#"if [ "$1" = "__lower" ]; then
  n=$(ls -A . 2>/dev/null | wc -l)
  printf '%s|cwd=%s|empty=%s\n' "$*" "$PWD" "$n" >> "$HOME/lower.log"
  printf '{"tool":"cigo","version":"1.17.0","cig":"1.25.0","engine":"vm","kept":true,"lowered":true,"reason":null}\n'
  exit 0
fi"#;

/// `__lower` that exits 2 as an older cigo would (an unknown subcommand, clap's usage code).
const LOWER_UNKNOWN: &str = r#"if [ "$1" = "__lower" ]; then
  printf 'unrecognized subcommand: __lower\n' >&2
  exit 2
fi"#;

/// `__lower` that prints something that is not JSON.
const LOWER_NONSENSE: &str = r#"if [ "$1" = "__lower" ]; then
  printf 'I am not JSON\n'
  exit 0
fi"#;

/// `__lower` that sleeps past the shortened limit (`CIGI_LOWER_TIMEOUT_SECS`).
const LOWER_SLEEP: &str = r#"if [ "$1" = "__lower" ]; then
  echo "started" > "$HOME/lower.started"
  sleep 20
  exit 0
fi"#;

/// Run the installer with `--yes`, the extra env, the arguments. Returns the output.
fn run(fx: &Fixture, env: &[(&str, &str)], args: &[&str]) -> Output {
    let mut c = fx.cigi();
    c.env("CIGI_ALLOW_SINGLE_SOURCE", "true").arg("--yes");
    for (k, v) in env {
        c.env(k, v);
    }
    c.args(args);
    cigi_testkit::output_retrying(&mut c).expect("run ciginstaller")
}

fn data(o: &Output) -> Value {
    let line = String::from_utf8_lossy(&o.stdout)
        .lines()
        .last()
        .unwrap_or("")
        .to_string();
    serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {}", text(o)))
}

fn log(fx: &Fixture) -> String {
    std::fs::read_to_string(fx.user_home.join("lower.log")).unwrap_or_default()
}

fn log_lines(fx: &Fixture) -> Vec<String> {
    log(fx)
        .lines()
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// A fixture with one carton that has cig, cigo and cigcompile; the cigo answers `__lower --json`
/// with the given shell snippet. The carton is published but not installed.
fn setup(cigo_hook: &str) -> Fixture {
    let mut fx = fixture();
    fx.product("cigo", "1.17.0").shell(cigo_hook);
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

// ------------------------------------------------------------------ install: cigo ran

#[test]
fn an_install_of_a_carton_with_cigo_runs_lower_once_in_an_empty_folder_outside_the_home() {
    let fx = setup(LOWER_OK);
    let o = run(
        &fx,
        &[],
        &["--json", "install", "carton", "--carton", "0.1.0"],
    );
    ok(&o);
    let lines = log_lines(&fx);
    assert_eq!(lines.len(), 1, "one `__lower` call: {lines:?}");
    let line = &lines[0];
    assert!(line.starts_with("__lower --json|"), "{line}");
    assert!(line.contains("|empty=0"), "cwd was empty: {line}");
    // the cwd was not the home or anything under it
    let cwd_str = line
        .split('|')
        .find_map(|p| p.strip_prefix("cwd="))
        .expect("cwd");
    let cwd = std::fs::canonicalize(Path::new(cwd_str))
        .unwrap_or_else(|_| Path::new(cwd_str).to_path_buf());
    let user_home = std::fs::canonicalize(&fx.user_home).unwrap_or_else(|_| fx.user_home.clone());
    let cig_home = std::fs::canonicalize(fx.cigscript()).unwrap_or_else(|_| fx.cigscript());
    assert!(
        !cwd.starts_with(&user_home) && !cwd.starts_with(&cig_home),
        "cwd {} must not be in the home ({})",
        cwd.display(),
        user_home.display()
    );
    // the installer tells the person, and carries the JSON document
    let d = data(&o);
    let detail = d["detail"].to_string();
    assert!(
        detail.contains("cig's VM from its next call"),
        "install's detail: {detail}"
    );
    assert_eq!(d["data"]["cigo_lower"]["engine"], "vm");
    assert_eq!(d["data"]["cigo_lower"]["kept"], true);
}

// ------------------------------------------------------------------ update: cig alone changed

#[test]
fn an_update_that_changed_cig_alone_runs_lower_once() {
    let mut fx = setup(LOWER_OK);
    // the first carton goes in, no lower log yet after we reset
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    let _ = std::fs::remove_file(fx.user_home.join("lower.log"));
    // the second carton bumps cig only; cigo stays at 1.17.0, cigcompile at 0.2.0
    fx.product("cig", "1.21.0");
    fx.carton(
        "0.2.0",
        &[
            ("cig", "1.21.0"),
            ("cigo", "1.17.0"),
            ("cigcompile", "0.2.0"),
        ],
    );
    fx.publish("stable");
    let o = run(&fx, &[], &["--json", "update", "--no-trial"]);
    ok(&o);
    let lines = log_lines(&fx);
    assert_eq!(lines.len(), 1, "one `__lower` call: {lines:?}");
    assert_eq!(data(&o)["data"]["cigo_lower"]["engine"], "vm");
}

// ------------------------------------------------------------------ no cig or cigo change

#[test]
fn an_update_that_changed_neither_cig_nor_cigo_does_not_run_lower() {
    let mut fx = setup(LOWER_OK);
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    let _ = std::fs::remove_file(fx.user_home.join("lower.log"));
    // bump cigcompile only
    fx.product("cigcompile", "0.3.0");
    fx.carton(
        "0.2.0",
        &[
            ("cig", "1.20.0"),
            ("cigo", "1.17.0"),
            ("cigcompile", "0.3.0"),
        ],
    );
    fx.publish("stable");
    ok(&run(&fx, &[], &["update", "--no-trial"]));
    assert_eq!(log(&fx), "", "no `__lower` call");
}

// ------------------------------------------------------------------ --dry-run

#[test]
fn an_install_dry_run_does_not_run_lower() {
    let fx = setup(LOWER_OK);
    let o = run(
        &fx,
        &[],
        &["install", "carton", "--carton", "0.1.0", "--dry-run"],
    );
    ok(&o);
    assert_eq!(log(&fx), "");
}

// ------------------------------------------------------------------ --check

#[test]
fn an_update_check_does_not_run_lower() {
    let mut fx = setup(LOWER_OK);
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    let _ = std::fs::remove_file(fx.user_home.join("lower.log"));
    fx.product("cig", "1.21.0");
    fx.carton(
        "0.2.0",
        &[
            ("cig", "1.21.0"),
            ("cigo", "1.17.0"),
            ("cigcompile", "0.2.0"),
        ],
    );
    fx.publish("stable");
    ok(&run(&fx, &[], &["update", "--check"]));
    assert_eq!(log(&fx), "");
}

// ------------------------------------------------------------------ older cigo (exit 2)

#[test]
fn an_older_cigo_that_answers_unknown_leaves_the_install_alone_and_is_silent() {
    let fx = setup(LOWER_UNKNOWN);
    let o = run(
        &fx,
        &[],
        &["--json", "install", "carton", "--carton", "0.1.0"],
    );
    ok(&o); // exit 0: the install committed, and the lower step never changes that
    let d = data(&o);
    // nothing shown to the person, nothing recorded in the --json field
    let detail = d["detail"].to_string();
    assert!(!detail.contains("cigo runs on cig"), "{detail}");
    assert!(d["data"]["cigo_lower"].is_null(), "{d:#}");
}

// ------------------------------------------------------------------ nonsense answer

#[test]
fn a_cigo_that_prints_nonsense_leaves_the_install_alone_and_a_note_in_its_place() {
    let fx = setup(LOWER_NONSENSE);
    let o = run(
        &fx,
        &[],
        &["--json", "install", "carton", "--carton", "0.1.0"],
    );
    ok(&o);
    let d = data(&o);
    assert!(d["data"]["cigo_lower"].is_null(), "{d:#}");
    let detail = d["detail"].to_string();
    assert!(
        detail.contains("cannot read") || detail.contains("not json") || detail.contains("shape"),
        "a note about the shape: {detail}"
    );
}

// ------------------------------------------------------------------ timeout

#[test]
fn a_cigo_that_sleeps_past_the_limit_is_killed_and_the_install_stays_ok() {
    let fx = setup(LOWER_SLEEP);
    // 1 second is plenty: the fake sleeps 20
    let o = run(
        &fx,
        &[("CIGI_LOWER_TIMEOUT_SECS", "1")],
        &["--json", "install", "carton", "--carton", "0.1.0"],
    );
    ok(&o);
    assert!(
        fx.user_home.join("lower.started").exists(),
        "`__lower` was started"
    );
    let d = data(&o);
    assert!(d["data"]["cigo_lower"].is_null(), "{d:#}");
    let detail = d["detail"].to_string();
    assert!(
        detail.contains("did not finish") || detail.contains("could not be run"),
        "a note about the timeout: {detail}"
    );
}

// ------------------------------------------------------------------ repair and self update

#[test]
fn a_repair_that_relinks_a_shim_runs_lower_once_and_one_with_nothing_to_do_does_not() {
    let fx = setup(LOWER_OK);
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    let _ = std::fs::remove_file(fx.user_home.join("lower.log"));
    // nothing is damaged: the repair changes nothing and does not run the step
    ok(&run(&fx, &[], &["repair"]));
    assert_eq!(
        log(&fx),
        "",
        "no `__lower` call from a repair with nothing to do"
    );
    // the cig shim is gone: the repair links it again, and cigo is told
    std::fs::remove_file(fx.cigscript().join("bin").join("cig")).unwrap();
    let o = run(&fx, &[], &["--json", "repair"]);
    ok(&o);
    let lines = log_lines(&fx);
    assert_eq!(lines.len(), 1, "one `__lower` call: {lines:?}");
    assert_eq!(data(&o)["data"]["cigo_lower"]["engine"], "vm");
}

#[test]
fn a_self_update_runs_lower_once_after_the_shims_follow_the_new_build() {
    let mut fx = fixture();
    fx.product("cigo", "1.17.0").shell(LOWER_OK);
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
    let calls: Vec<&str> = seen.lines().filter(|l| l.starts_with("cigo ")).collect();
    assert_eq!(calls, ["cigo __lower --json"], "{seen}");
    let d = data(&o);
    assert!(d["data"]["cigo_lower"].is_null(), "{d:#}");
    assert!(!d["detail"].to_string().contains("__lower"), "{d:#}");
}
