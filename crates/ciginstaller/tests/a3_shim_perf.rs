// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `a3_shim_perf` (SPEC 14.2, 14.3): the shim's own time from process start to `exec`, measured by
//! `CIGI_SHIM_TIMING=1`, against an installed fixture home. The budget is 2 ms (#20). On the debug
//! build a plain `cargo test` gets it asserts with headroom (5 ms); on a release build
//! (`cargo test --release`, which the `bench` workflow runs) it asserts the budget itself. The
//! strict 1 ms assertion on the release shim is in cigi-shim's own tests.
#![cfg(unix)]

mod common;
use std::process::Command;

use common::*;

#[test]
fn a3_shim_perf() {
    let mut fx = fixture();
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.16.0")]);
    fx.publish("stable");
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    let base = fx.cigi();
    let mut own: Vec<u128> = Vec::new();
    for _ in 0..200 {
        let mut c = Command::new(fx.cigscript().join("bin/cig"));
        c.env_clear();
        for (k, v) in base.get_envs() {
            if let Some(v) = v {
                c.env(k, v);
            }
        }
        c.env("CIGI_SHIM_TIMING", "1")
            .env_remove("CIGI_CARTON")
            .current_dir(&fx.project);
        let mut tries = 0;
        let o = loop {
            match c.output() {
                Err(e) if e.raw_os_error() == Some(26) && tries < 200 => {
                    tries += 1;
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                r => break r.expect("run the shim"),
            }
        };
        assert!(o.status.success(), "{}", text(&o));
        let err = String::from_utf8_lossy(&o.stderr);
        let ns = err
            .lines()
            .find_map(|l| l.strip_prefix("cigi-shim-timing-ns: "))
            .unwrap_or_else(|| panic!("no timing line: {err}"))
            .trim()
            .parse::<u128>()
            .unwrap();
        own.push(ns);
    }
    own.sort_unstable();
    let (med, p95) = (own[own.len() / 2], own[own.len() * 95 / 100]);
    let (limit_ns, build) = if cfg!(debug_assertions) {
        (5_000_000, "debug, 5 ms headroom")
    } else {
        (2_000_000, "release, the 2 ms budget")
    };
    eprintln!(
        "a3_shim_perf: shim-own median {med} ns, p95 {p95} ns over {} runs ({build})",
        own.len()
    );
    assert!(
        med < limit_ns,
        "shim median {med} ns is over the limit of {limit_ns} ns ({build})"
    );
}
