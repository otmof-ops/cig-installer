// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `b1_timings` (#35, cig-script#179): the installer writes its phases to the descriptor
//! `CIG_TIMINGS_FD` names as timings lines (format 1), and only when it is set. Unix only: a
//! descriptor is handed to a child as `9>file` by a shell.
#![cfg(unix)]

mod common;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use cigi_testkit::Fixture;
use common::*;
use serde_json::Value;

/// The installer under `sh`, with descriptor 9 going to `out` and `fd` as `CIG_TIMINGS_FD`
/// (when given). A hard timeout: nothing here may wait on a descriptor.
fn with_fd(fx: &Fixture, fd: Option<&str>, out: &std::path::Path, args: &[&str]) -> (Output, f64) {
    let base = fx.cigi();
    let mut c = Command::new("timeout");
    c.arg("60")
        .arg("sh")
        .arg("-c")
        .arg(r#"out=$1; shift; exec "$@" 9>"$out""#);
    c.arg("sh")
        .arg(out)
        .arg(base.get_program())
        .args(base.get_args())
        .arg("--yes")
        .args(args);
    for (k, v) in base.get_envs() {
        match v {
            Some(v) => c.env(k, v),
            None => c.env_remove(k),
        };
    }
    if let Some(dir) = base.get_current_dir() {
        c.current_dir(dir);
    }
    c.env_remove("CIG_TIMINGS_FD");
    if let Some(fd) = fd {
        c.env("CIG_TIMINGS_FD", fd);
    }
    let t = Instant::now();
    let o = c.output().expect("run ciginstaller under sh");
    (o, t.elapsed().as_secs_f64() * 1000.0)
}

fn lines(out: &std::path::Path) -> Vec<Value> {
    std::fs::read_to_string(out)
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("{e}: {l}")))
        .collect()
}

#[test]
fn b1_phases_go_to_the_descriptor_in_format_1() {
    let fx = fixture();
    let out = fx.tmp.path().join("phases.jsonl");
    let (o, wall) = with_fd(&fx, Some("9"), &out, &["status"]);
    ok(&o);
    let got = lines(&out);
    let names: Vec<&str> = got.iter().map(|l| l["phase"].as_str().unwrap()).collect();
    assert_eq!(names, ["start", "home", "command", "output"], "{got:?}");
    let mut sum = 0.0;
    for l in &got {
        assert_eq!(l["v"], 1);
        assert_eq!(l["product"], "ciginstaller");
        let ms = l["ms"].as_f64().expect("ms is a number");
        assert!(ms >= 0.0, "{l}");
        sum += ms;
    }
    // the phases are inside the process: never more than what the whole run took, shell included
    assert!(sum > 0.0 && sum <= wall, "phases {sum} ms, wall {wall} ms");
    assert!(Duration::from_secs(60).as_secs_f64() * 1000.0 > wall);
}

#[test]
fn b1_nothing_is_written_when_the_variable_is_unset_or_wrong() {
    let fx = fixture();
    for (i, fd) in [None, Some(""), Some("1"), Some("9;ls"), Some("x")]
        .into_iter()
        .enumerate()
    {
        let out = fx.tmp.path().join(format!("none-{i}.jsonl"));
        let (o, _) = with_fd(&fx, fd, &out, &["status"]);
        ok(&o);
        assert!(lines(&out).is_empty(), "{fd:?} wrote lines");
    }
}

#[test]
fn b1_a_descriptor_that_is_not_open_is_ignored() {
    let fx = fixture();
    let out = fx.tmp.path().join("closed.jsonl");
    // 7 is not open in the child: the command still succeeds and says what it always says
    let (o, _) = with_fd(&fx, Some("7"), &out, &["status"]);
    ok(&o);
    assert!(text(&o).contains("nothing is installed"), "{}", text(&o));
    assert!(lines(&out).is_empty());
}
