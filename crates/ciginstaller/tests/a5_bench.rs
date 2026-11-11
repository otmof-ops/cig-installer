// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `a5_bench` (#35): `ciginstaller bench` measures the installer's own budgets here, locally and
//! read-only, compares them with `bench/budgets.toml`, and says which views are not built. The
//! timings are asserted with headroom: these run on the debug build a test gets, so the numbers
//! are checked for being there and sane, and the strict budgets are the release build's.
#![cfg(unix)]

mod common;
use std::process::Output;

use cigi_testkit::{Fixture, Snapshot};
use common::*;
use serde_json::Value;

fn json(o: &Output) -> Value {
    serde_json::from_slice(&o.stdout).unwrap_or_else(|e| panic!("{e}: {}", text(o)))
}

fn installed() -> Fixture {
    let mut fx = fixture();
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.17.0")]);
    fx.publish("stable");
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    fx
}

fn bench(fx: &Fixture, args: &[&str]) -> Value {
    let mut a = vec!["--json", "bench", "--runs", "20"];
    a.extend_from_slice(args);
    let o = cigi(fx, &a);
    ok(&o);
    json(&o)["data"].clone()
}

fn row<'a>(d: &'a Value, id: &str) -> &'a Value {
    d["budgets"]
        .as_array()
        .and_then(|b| b.iter().find(|r| r["id"] == id))
        .unwrap_or_else(|| panic!("no budget row {id} in {d}"))
}

#[test]
fn a5_bench_measures_every_budget_it_can_in_the_reserved_shape() {
    let fx = installed();
    let d = bench(&fx, &[]);
    // the shape docs/JSON.md reserves, every key there
    for k in [
        "shim_dispatch_ns",
        "check_ms",
        "switch_ms",
        "shim_bytes",
        "installer_bytes",
    ] {
        assert!(d[k].is_number(), "{k} is not a number in {d}");
    }
    let (shim_ns, check, switch) = (
        d["shim_dispatch_ns"].as_u64().unwrap(),
        d["check_ms"].as_f64().unwrap(),
        d["switch_ms"].as_f64().unwrap(),
    );
    eprintln!("a5_bench: shim {shim_ns} ns, check {check} ms, switch {switch} ms (debug build)");
    assert!(shim_ns > 0 && shim_ns < 5_000_000, "shim {shim_ns} ns");
    assert!(check > 0.0 && check < 1000.0, "check {check} ms");
    assert!(switch > 0.0 && switch < 1000.0, "switch {switch} ms");
    // the p95 rides beside each median, never below it
    assert!(d["shim_dispatch_p95_ns"].as_u64().unwrap() >= shim_ns);
    assert!(d["check_p95_ms"].as_f64().unwrap() >= check);
    assert!(d["switch_p95_ms"].as_f64().unwrap() >= switch);
    assert!(d["shim_bytes"].as_u64().unwrap() > 0);
    assert!(d["installer_bytes"].as_u64().unwrap() > 0);
    assert_eq!(d["runs"], 20);
    assert_eq!(
        d["build"],
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    );
}

#[test]
fn a5_bench_compares_each_row_with_the_budget_in_bench_toml() {
    let fx = installed();
    let d = bench(&fx, &[]);
    for (id, limit) in [
        ("shim_dispatch", 2.0),
        ("check", 200.0),
        ("switch", 100.0),
        ("disk_five_cigo", 1.5),
        ("installer_bytes", 10_000_000.0),
        ("shim_bytes", 500_000.0),
        ("install_pack_dev", 30_000.0),
        ("install_carton", 60_000.0),
    ] {
        assert_eq!(row(&d, id)["limit"].as_f64(), Some(limit), "{id}");
    }
    // a median over its limit says so, and the count agrees with the rows
    let over = d["budgets"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["status"] == "over")
        .count();
    assert_eq!(d["over_budget"].as_u64(), Some(over as u64));
    for r in d["budgets"].as_array().unwrap() {
        if let (Some(m), Some(l)) = (r["median"].as_f64(), r["limit"].as_f64()) {
            if r["status"] == "ok" {
                assert!(m <= l, "{r}");
            }
            if r["status"] == "over" {
                assert!(m > l, "{r}");
            }
        }
    }
    // the debug build's sizes are shown and not judged
    if cfg!(debug_assertions) {
        assert_eq!(row(&d, "installer_bytes")["status"], "info");
        assert_eq!(row(&d, "shim_bytes")["status"], "info");
    }
    // what needs a network or a mirror is named, not silently dropped
    assert_eq!(row(&d, "install_carton")["status"], "not measured");
    let by_test = d["not_measured_by_bench"].as_array().unwrap();
    assert!(by_test.iter().any(|b| b["id"] == "update_check"), "{d}");
}

#[test]
fn a5_bench_is_read_only_and_sends_nothing() {
    let fx = installed();
    let server = fx.serve();
    let scratch = fx.tmp.path().join("scratch");
    let before = Snapshot::take(&fx.user_home);
    let o = fx
        .cigi_with_mirror(&server.url())
        .env("CIGI_ALLOW_SINGLE_SOURCE", "true")
        .args(["--yes", "--json", "bench", "--runs", "10", "--scratch"])
        .arg(&scratch)
        .output()
        .unwrap();
    ok(&o);
    // the home, the profile and every other file of the user's are as they were
    assert_eq!(
        before.diff(&Snapshot::take(&fx.user_home)),
        Vec::<String>::new()
    );
    assert!(server.requests().is_empty(), "{:?}", server.paths());
    // the scratch home was made in the directory asked for, and removed
    assert!(scratch.is_dir());
    assert_eq!(std::fs::read_dir(&scratch).unwrap().count(), 0);
}

#[test]
fn a5_bench_on_an_empty_home_measures_what_needs_no_install_and_says_the_rest() {
    let fx = fixture();
    let o = cigi(&fx, &["--json", "bench", "--runs", "10"]);
    ok(&o);
    let d = json(&o);
    // a busy disk can put the switch over its budget; the next step then is the code's page
    assert!(
        d["next"] == "ciginstaller install carton" || d["next"] == "ciginstaller explain E1866",
        "{d}"
    );
    let d = &d["data"];
    assert!(d["shim_dispatch_ns"].is_number());
    assert!(d["switch_ms"].is_number());
    assert!(d["check_ms"].is_null());
    assert_eq!(row(d, "check")["status"], "not measured");
    assert!(
        row(d, "check")["note"]
            .as_str()
            .unwrap()
            .contains("no cached manifest"),
        "{d}"
    );
    // nothing was created in the real home
    assert!(
        !fx.cigscript().exists(),
        "bench made the home: {:?}",
        Snapshot::take(&fx.cigscript())
            .entries
            .keys()
            .collect::<Vec<_>>()
    );
}

#[test]
fn a5_bench_says_which_views_are_not_built_and_why() {
    let fx = installed();
    let o = cigi(&fx, &["bench", "--plain", "--runs", "5"]);
    ok(&o);
    assert!(text(&o).contains("not built yet"), "{}", text(&o));
    assert!(
        text(&o).contains("ciginstaller explain E1867"),
        "{}",
        text(&o)
    );
    let d = bench(&fx, &[]);
    let nb = d["not_built"].as_array().unwrap();
    assert!(nb.len() >= 5);
    let all = serde_json::to_string(nb).unwrap();
    for words in ["cig-script#179", "#30", "#33", "#29", "(the tool)"] {
        assert!(all.contains(words), "{words} not in {all}");
    }
}

#[test]
fn a5_bench_flags_that_are_not_built_refuse_with_e1867_and_change_nothing() {
    let fx = installed();
    let before = Snapshot::take(&fx.cigscript());
    for args in [
        vec!["--watch"],
        vec!["--carton", "0.1.0"],
        vec!["--with", "cig=1.16.0"],
        vec!["--matrix", "carton=0.1.0,0.1.1"],
        vec!["--product", "cigo"],
        vec!["--only", "hook"],
    ] {
        let mut a = vec!["bench"];
        a.extend(args.iter().copied());
        let o = cigi(&fx, &a);
        assert_eq!(o.status.code(), Some(3), "{args:?}: {}", text(&o));
        let t = text(&o);
        assert!(t.contains("E1867"), "{args:?}: {t}");
        assert!(t.contains("not built yet"), "{args:?}: {t}");
        assert!(t.contains("otmof-ops/cig-script#179"), "{args:?}: {t}");
    }
    assert_eq!(
        before.diff(&Snapshot::take(&fx.cigscript())),
        Vec::<String>::new()
    );
    // the page says what each view waits on
    let o = cigi(&fx, &["explain", "E1867"]);
    ok(&o);
    for words in ["#179", "#30", "#33", "#29"] {
        assert!(text(&o).contains(words), "{words}: {}", text(&o));
    }
}

#[test]
fn a5_bench_strict_passes_when_nothing_is_over() {
    // the release budgets are not asserted here; a build that is over says E1866 (unit-tested
    // in flows::bench), and one that is not exits 0
    let fx = installed();
    let o = cigi(&fx, &["--json", "bench", "--runs", "10", "--strict"]);
    let d = json(&o);
    if d["ok"] == true {
        assert_eq!(o.status.code(), Some(0));
    } else {
        assert_eq!(o.status.code(), Some(1), "{}", text(&o));
        assert_eq!(d["error"]["code"], "E1866", "{d}");
    }
}

#[test]
fn a5_bench_from_a_mirror_times_the_installs_in_a_scratch_home() {
    let mut fx = fixture();
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.17.0")]);
    fx.publish("stable");
    let mirror = fx.releases.clone();
    let before = Snapshot::take(&fx.user_home);
    let o = cigi(
        &fx,
        &[
            "--json",
            "bench",
            "--runs",
            "5",
            "--from",
            mirror.to_str().unwrap(),
        ],
    );
    ok(&o);
    let d = json(&o)["data"].clone();
    for (id, key) in [
        ("install_pack_dev", "install_pack_dev_ms"),
        ("install_carton", "install_carton_ms"),
    ] {
        assert_eq!(row(&d, id)["status"], "ok", "{id}: {}", row(&d, id));
        let ms = d[key].as_f64().unwrap();
        assert!(ms > 0.0 && ms < 30_000.0, "{id} {ms} ms");
    }
    // the real home and the user's files were not touched by the installs
    assert_eq!(
        before.diff(&Snapshot::take(&fx.user_home)),
        Vec::<String>::new()
    );
}

// ---- after an update ------------------------------------------------------------------------

fn two(fx: &mut Fixture) {
    fx.product("cig", "1.20.0").dry_run_output("plan: 1 op");
    fx.product("cig", "1.21.0").dry_run_output("plan: 1 op");
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.17.0")]);
    fx.carton("0.1.1", &[("cig", "1.21.0"), ("cigo", "1.17.0")]);
    fx.publish("stable");
}

fn last(fx: &Fixture) -> std::path::PathBuf {
    fx.cigscript().join("installer/bench/last.json")
}

#[test]
fn after_update_check_is_off_by_default_and_local() {
    // off by default: an update measures nothing, says nothing about it and writes nothing
    let mut off = fixture();
    two(&mut off);
    ok(&cigi(&off, &["install", "carton", "--carton", "0.1.0"]));
    assert_eq!(
        json(&cigi(
            &off,
            &["--json", "config", "get", "bench_after_update"]
        ))["data"]["value"],
        "off"
    );
    let server = off.serve_tls();
    let o = off
        .cigi_with_server(&server)
        .env("CIGI_ALLOW_SINGLE_SOURCE", "true")
        .args(["--yes", "update"])
        .output()
        .unwrap();
    ok(&o);
    assert!(!text(&o).contains("local check"), "{}", text(&o));
    assert!(!off.cigscript().join("installer/bench").exists());
    let requests_off = server.paths();

    // on: a line after the update, the numbers kept here, and not one request more
    let mut on = fixture();
    two(&mut on);
    ok(&cigi(&on, &["install", "carton", "--carton", "0.1.0"]));
    ok(&cigi(&on, &["config", "set", "bench_after_update", "on"]));
    let server = on.serve_tls();
    let o = on
        .cigi_with_server(&server)
        .env("CIGI_ALLOW_SINGLE_SOURCE", "true")
        .args(["--yes", "update"])
        .output()
        .unwrap();
    ok(&o);
    let t = text(&o);
    assert!(t.contains("local check after the update"), "{t}");
    assert!(t.contains("nothing was sent anywhere"), "{t}");
    assert_eq!(server.paths(), requests_off);
    server
        .assert_no_telemetry()
        .unwrap_or_else(|v| panic!("{v:?}"));
    let rec: Value = serde_json::from_str(&std::fs::read_to_string(last(&on)).unwrap()).unwrap();
    assert!(rec["shim_dispatch_ns"].is_number() && rec["check_ms"].is_number());
    // `--json` stays one object, and `update --check` measures nothing
    std::fs::remove_file(last(&on)).unwrap();
    let o = cigi(&on, &["--json", "update", "--check"]);
    ok(&o);
    assert!(!last(&on).exists());
}

#[test]
fn after_update_check_shows_a_regression_once_and_names_rollback() {
    let mut fx = fixture();
    two(&mut fx);
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    ok(&cigi(&fx, &["config", "set", "bench_after_update", "on"]));
    // a baseline far faster than any machine: whatever the update measures is slower
    std::fs::create_dir_all(fx.cigscript().join("installer/bench")).unwrap();
    std::fs::write(
        last(&fx),
        r#"{"format":1,"shim_dispatch_ns":1.0,"check_ms":0.01,"switch_ms":0.01}"#,
    )
    .unwrap();
    let o = cigi(&fx, &["update"]);
    ok(&o);
    let t = text(&o);
    let new: Value = serde_json::from_str(&std::fs::read_to_string(last(&fx)).unwrap()).unwrap();
    // the same rule the flow applies: a fifth and more than the noise floor
    let slower = new["check_ms"].as_f64().unwrap() - 0.01 > 5.0
        || new["switch_ms"].as_f64().unwrap() - 0.01 > 5.0
        || new["shim_dispatch_ns"].as_f64().unwrap() - 1.0 > 100_000.0;
    assert_eq!(t.contains("since the last local sample"), slower, "{t}");
    if slower {
        assert!(t.contains("slower here"), "{t}");
        assert!(
            t.contains("ciginstaller rollback goes back to carton 0.1.0"),
            "{t}"
        );
    }
    // the new sample is the baseline now: the same numbers are not a regression twice
    assert!(new["check_ms"].as_f64().unwrap() > 0.01);
}
