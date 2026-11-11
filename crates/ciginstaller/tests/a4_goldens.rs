// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! Goldens of the real binary (#32): `status`, `which --why`, `config list`, `history` and
//! `doctor` (healthy, and with a deleted shim), each in the `--plain`, `--no-color` and `--json`
//! variants, run on a fixture home. The `cigi-ui` goldens are drawn from synthetic reports; these
//! are what the program says.
//!
//! The fixture's paths become `~/.cigscript` and `~`, times become `<time>` and transaction ids
//! `<id>`. Regenerate with `UPDATE_GOLDEN=1 cargo test -p ciginstaller --test a4_goldens`, then
//! read the diff: a change here is a change to what people see.
#![cfg(unix)]

mod common;

use std::path::PathBuf;

use cigi_testkit::Fixture;
use common::*;

const VARIANTS: [(&str, &str); 3] = [
    ("plain", "--plain"),
    ("no-color", "--no-color"),
    ("json", "--json"),
];

/// A machine that has been installed on 0.1.0, updated to 0.1.1, and has 0.1.2 waiting in its
/// cached manifest.
fn machine() -> Fixture {
    let mut fx = fixture();
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.17.0")]);
    fx.publish("stable");
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    fx.carton("0.1.1", &[("cig", "1.20.1"), ("cigo", "1.17.0")]);
    fx.publish("stable");
    ok(&cigi(&fx, &["update", "--no-trial"]));
    fx.carton("0.1.2", &[("cig", "1.20.2"), ("cigo", "1.17.1")]);
    fx.publish("stable");
    ok(&cigi(&fx, &["update", "--check", "--refresh"]));
    fx
}

/// `20dd-dd-ddTdd:dd:dd` with an optional fraction and `Z`, as `<time>`.
fn times(s: &str) -> String {
    let b: Vec<char> = s.chars().collect();
    let shape = "dddd-dd-ddTdd:dd:dd";
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        let fits = i + shape.len() <= b.len()
            && shape.chars().enumerate().all(|(k, p)| {
                let c = b[i + k];
                match p {
                    'd' => c.is_ascii_digit(),
                    _ => c == p,
                }
            });
        if fits {
            i += shape.len();
            if b.get(i) == Some(&'.') {
                i += 1;
                while b.get(i).is_some_and(|c| c.is_ascii_digit()) {
                    i += 1;
                }
            }
            if b.get(i) == Some(&'Z') {
                i += 1;
            }
            out.push_str("<time>");
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    out
}

/// Sizes such as `24.4 MiB` as `<size>`: they follow the fixture's builds, which differ by
/// profile, toolchain and platform, so a golden that kept them would only pass on one machine.
fn sizes(s: &str) -> String {
    const UNITS: [&str; 5] = ["TiB", "GiB", "MiB", "KiB", "B"];
    let b: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        let starts = b[i].is_ascii_digit() && (i == 0 || !b[i - 1].is_ascii_alphanumeric());
        if starts {
            let mut j = i;
            while b.get(j).is_some_and(|c| c.is_ascii_digit()) {
                j += 1;
            }
            if b.get(j) == Some(&'.') && b.get(j + 1).is_some_and(|c| c.is_ascii_digit()) {
                j += 1;
                while b.get(j).is_some_and(|c| c.is_ascii_digit()) {
                    j += 1;
                }
            }
            if b.get(j) == Some(&' ') {
                let rest: String = b[j + 1..].iter().take(3).collect();
                if let Some(u) = UNITS.iter().find(|u| rest.starts_with(**u)) {
                    let end = j + 1 + u.len();
                    if !b.get(end).is_some_and(|c| c.is_ascii_alphanumeric()) {
                        out.push_str("<size>");
                        i = end;
                        continue;
                    }
                }
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// The ids of transactions: the history's `id` and `txn` and the lines that name them.
fn ids(s: &str, known: &[String]) -> String {
    let mut s = s.to_string();
    for k in known {
        s = s.replace(k.as_str(), "<id>");
    }
    s
}

fn txn_ids(fx: &Fixture) -> Vec<String> {
    let state = std::fs::read_to_string(fx.cigscript().join("installer/state.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&state).unwrap();
    let mut out = Vec::new();
    for e in v["history"].as_array().into_iter().flatten() {
        for k in ["id", "txn"] {
            if let Some(s) = e[k].as_str() {
                out.push(s.to_string());
            }
        }
    }
    out.sort_by_key(|s| std::cmp::Reverse(s.len()));
    out.dedup();
    out
}

fn normalize(fx: &Fixture, s: &str) -> String {
    let s = s
        .replace(fx.cigscript().to_str().unwrap_or("?"), "~/.cigscript")
        .replace(fx.project.to_str().unwrap_or("?"), "<project>")
        .replace(fx.user_home.to_str().unwrap_or("?"), "~")
        .replace(fx.tmp.path().to_str().unwrap_or("?"), "<tmp>");
    sizes(&times(&ids(&s, &txn_ids(fx)))).replace('\x1b', "␛")
}

/// The kernel files doctor's guard probes read, fixed, so no golden depends on the machine it was
/// made on (`CIGI_DOCTOR_PROBE_ROOT` only changes where doctor reads from).
fn probe_root(fx: &Fixture) -> PathBuf {
    let root = fx.tmp.path().join("probe-root");
    for (rel, body) in [
        ("proc/sys/kernel/osrelease", "6.8.0-1-generic\n"),
        ("proc/sys/user/max_user_namespaces", "31000\n"),
        (
            "sys/kernel/security/lsm",
            "lockdown,capability,yama,apparmor\n",
        ),
    ] {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }
    root
}

fn run(fx: &Fixture, variant: &str, args: &[&str]) -> String {
    run_on(fx, variant, args, "file:///mirror")
}

fn run_on(fx: &Fixture, variant: &str, args: &[&str], mirror: &str) -> String {
    let mut c = fx.cigi();
    c.env("CIGI_DOCTOR_PROBE_ROOT", probe_root(fx));
    // a fixed mirror, so no fixture path (and no cut-short one) reaches a golden
    c.env("CIGI_ALLOW_SINGLE_SOURCE", "true")
        .env("CIGI_MIRROR", mirror)
        .arg(variant)
        .args(args);
    let o = loop {
        match c.output() {
            Err(e) if e.raw_os_error() == Some(26) => {
                std::thread::sleep(std::time::Duration::from_millis(5))
            }
            r => break r.expect("run ciginstaller"),
        }
    };
    let got = format!(
        "--- exit {} ---\n--- stdout ---\n{}--- stderr ---\n{}",
        o.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    normalize(fx, &got)
}

fn check(fx: &Fixture, name: &str, args: &[&str]) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    for (variant, flag) in VARIANTS {
        let got = run(fx, flag, args);
        let path = dir.join(format!("{name}.{variant}.txt"));
        if std::env::var_os("UPDATE_GOLDEN").is_some() {
            std::fs::write(&path, &got).unwrap();
            continue;
        }
        let want = std::fs::read_to_string(&path)
            .unwrap_or_else(|_| panic!("missing golden {}", path.display()));
        assert_eq!(got, want, "golden mismatch: {name}.{variant}");
    }
}

#[test]
fn a4_golden_status() {
    check(&machine(), "real-status", &["status"]);
}

#[test]
fn a4_golden_which_why_with_no_tool() {
    check(&machine(), "real-which-why", &["which", "--why"]);
}

#[test]
fn a4_golden_config_list() {
    check(&machine(), "real-config-list", &["config", "list"]);
}

#[test]
fn a4_golden_history() {
    check(&machine(), "real-history", &["history"]);
}

#[test]
fn a4_golden_doctor_healthy() {
    check(&machine(), "real-doctor-healthy", &["doctor"]);
}

#[test]
fn a4_golden_doctor_with_a_deleted_shim() {
    let fx = machine();
    std::fs::remove_file(fx.cigscript().join("bin/cig")).unwrap();
    check(&fx, "real-doctor-deleted-shim", &["doctor"]);
}

#[test]
fn a4_golden_update_no_trial() {
    // `update --no-trial` says it once: `trial: skipped (--no-trial)`, never "trial skipped: trial skipped"
    // the update changes the machine, so each form runs on its own fresh one (0.1.2 waiting)
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    for (variant, flag) in VARIANTS {
        let fx = machine();
        let got = run_on(&fx, flag, &["update", "--no-trial"], &fx.mirror_file_url());
        assert!(!got.contains("trial skipped"), "{got}");
        let path = dir.join(format!("real-update-no-trial.{variant}.txt"));
        if std::env::var_os("UPDATE_GOLDEN").is_some() {
            std::fs::write(&path, &got).unwrap();
            continue;
        }
        let want = std::fs::read_to_string(&path)
            .unwrap_or_else(|_| panic!("missing golden {}", path.display()));
        assert_eq!(got, want, "golden mismatch: real-update-no-trial.{variant}");
    }
}
