// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! The budgets of #20 that the other budget tests do not cover (the shim is in
//! `a3_shim_perf.rs`, the first lines in `budget.rs`, `install pack dev` in `a2_perf_install`).
//! They run on the debug build a test gets, so each limit has a debug figure with headroom and
//! the budget itself for a release build; timings are the best of several runs, because a busy
//! disk slows one run and a real regression slows all of them. Every number is printed.
#![cfg(unix)]

mod common;
use std::path::Path;
use std::time::Instant;

use cigi_testkit::Fixture;
use common::*;

fn limit_ms(budget: f64, debug: f64) -> f64 {
    if cfg!(debug_assertions) {
        debug
    } else {
        budget
    }
}

fn best_ms(runs: usize, mut f: impl FnMut()) -> f64 {
    (0..runs)
        .map(|_| {
            let t = Instant::now();
            f();
            t.elapsed().as_secs_f64() * 1000.0
        })
        .fold(f64::MAX, f64::min)
}

fn two_installed() -> Fixture {
    let mut fx = fixture();
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.16.0")]);
    fx.carton("0.2.0", &[("cig", "1.21.0"), ("cigo", "1.17.0")]);
    fx.publish("stable");
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.2.0"]));
    fx
}

/// `ciginstaller check` against cached metadata: under 200 ms.
#[test]
fn budget_check_with_cached_metadata() {
    let fx = two_installed();
    let took = best_ms(5, || ok(&cigi(&fx, &["--offline", "check"])));
    let limit = limit_ms(200.0, 1500.0);
    eprintln!("budget_check: best of 5 {took:.1} ms (limit {limit} ms; process start included)");
    assert!(took < limit, "check took {took:.1} ms, limit {limit} ms");
}

/// An atomic switch to another installed carton: under 100 ms.
#[test]
fn budget_switch_to_another_carton() {
    let fx = two_installed();
    let mut to = ["0.1.0", "0.2.0"].into_iter().cycle();
    let took = best_ms(7, || ok(&cigi(&fx, &["use", to.next().unwrap_or("0.1.0")])));
    let limit = limit_ms(100.0, 1500.0);
    eprintln!("budget_switch: best of 7 {took:.1} ms (limit {limit} ms; process start included)");
    assert!(took < limit, "a switch took {took:.1} ms, limit {limit} ms");
}

/// Installing a whole carton: under 60 s on broadband. From a local mirror the fixture bounds
/// the installer's own work (fetch, verify, store, link) at a tenth of that.
#[test]
fn budget_install_a_whole_carton() {
    let mut best = f64::MAX;
    for _ in 0..3 {
        let mut fx = fixture();
        fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.17.0")]);
        fx.publish("stable");
        let t = Instant::now();
        ok(&cigi(&fx, &["install", "carton"]));
        best = best.min(t.elapsed().as_secs_f64());
        if best < 3.0 {
            break;
        }
    }
    eprintln!(
        "budget_install_carton: best {best:.2} s from a local mirror (budget 60 s on broadband)"
    );
    assert!(
        best < 6.0,
        "the fastest of three carton installs took {best:.2} s"
    );
}

fn store_bytes(fx: &Fixture) -> u64 {
    fn walk(p: &Path) -> u64 {
        std::fs::read_dir(p)
            .map(|rd| {
                rd.flatten()
                    .map(|e| match e.metadata() {
                        Ok(m) if m.is_dir() => walk(&e.path()),
                        Ok(m) => m.len(),
                        Err(_) => 0,
                    })
                    .sum()
            })
            .unwrap_or(0)
    }
    walk(&fx.cigscript().join("store/objects"))
}

/// Five cigo versions cost under 1.5 times one: content-addressed toolchains (#1). cigo is
/// mostly files that do not change between versions, which the fixture's cigo mimics with one
/// shared 64 KB file beside the few bytes that differ.
#[test]
fn budget_disk_for_five_cigo_versions() {
    let shared: Vec<u8> = (0..65_536u32)
        .map(|i| (i.wrapping_mul(2654435761) >> 13) as u8)
        .collect();
    let mut fx = fixture();
    let versions = ["1.17.0", "1.18.0", "1.19.0", "1.20.0", "1.21.0"];
    for (i, v) in versions.iter().enumerate() {
        fx.product("cigo", v).file("data/shared.bin", &shared);
        fx.carton(&format!("0.1.{i}"), &[("cig", "1.20.0"), ("cigo", v)]);
    }
    fx.publish("stable");
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    let one = store_bytes(&fx);
    for i in 1..versions.len() {
        ok(&cigi(
            &fx,
            &["install", "carton", "--carton", &format!("0.1.{i}")],
        ));
    }
    let five = store_bytes(&fx);
    let ratio = five as f64 / one as f64;
    eprintln!(
        "budget_disk: one version {one} bytes, five {five} bytes, ratio {ratio:.3} (budget 1.5)"
    );
    assert!(ratio < 1.5, "five cigo versions cost {ratio:.2}x one");
    // `bench` says the same from the receipts, and judges it (five are held)
    let o = cigi(&fx, &["--json", "bench", "--runs", "5"]);
    ok(&o);
    let d: serde_json::Value = serde_json::from_slice(&o.stdout).expect("json");
    let r = d["data"]["disk_five_cigo_ratio"].as_f64().expect("a ratio");
    assert!((1.0..1.5).contains(&r), "bench says {r}");
    let row = d["data"]["budgets"]
        .as_array()
        .and_then(|b| b.iter().find(|x| x["id"] == "disk_five_cigo"))
        .expect("the row");
    assert_eq!(row["status"], "ok", "{row}");
}

/// A background update check: under 1 s, at most daily. Against the loopback fixture server the
/// installer's own part is bounded, and a second check within a day asks nothing.
#[test]
fn budget_update_check_under_a_second_and_at_most_daily() {
    let mut fx = fixture();
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.17.0")]);
    fx.carton("0.1.1", &[("cig", "1.20.0"), ("cigo", "1.17.0")]);
    fx.publish("stable");
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    let server = fx.serve();
    let run = |args: &[&str]| {
        let o = fx
            .cigi_with_mirror(&server.url())
            .env("CIGI_ALLOW_SINGLE_SOURCE", "true")
            .arg("--yes")
            .args(args)
            .output()
            .unwrap();
        assert!(
            o.status.success() || o.status.code() == Some(10),
            "{}",
            text(&o)
        );
    };
    let took = best_ms(5, || run(&["update", "--check", "--refresh"]));
    let limit = limit_ms(1000.0, 2500.0);
    eprintln!("budget_update_check: best of 5 {took:.1} ms on loopback (limit {limit} ms)");
    assert!(took < limit, "an update check took {took:.1} ms");
    server.clear();
    run(&["update", "--check"]);
    run(&["update", "--check"]);
    assert!(
        server.requests().is_empty(),
        "a check within a day asked a server: {:?}",
        server.paths()
    );
    server
        .assert_no_telemetry()
        .unwrap_or_else(|v| panic!("{v:?}"));
}

/// The installer under 10 MB and the shim under 500 KB: a release build's figures, the same
/// ones `ci/size-check.sh` enforces on the shipped files.
#[test]
fn budget_binary_sizes_on_a_release_build() {
    if cfg!(debug_assertions) {
        eprintln!("budget_sizes: a debug build; ci/size-check.sh judges the release files");
        return;
    }
    let fx = fixture();
    let size = |n: &str| {
        std::fs::metadata(cigi_testkit::binary_path(n))
            .map(|m| m.len())
            .unwrap_or(u64::MAX)
    };
    let _ = &fx;
    assert!(
        size("ciginstaller") < 10_000_000,
        "{}",
        size("ciginstaller")
    );
    assert!(size("cigi-shim") < 500_000, "{}", size("cigi-shim"));
}
