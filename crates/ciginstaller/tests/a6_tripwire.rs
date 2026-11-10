// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! The installer's tripwires (#28 part 1, its row of the table): after an `update`, every shim
//! resolves to the carton's build and the index says what `state.json` says; after a `rollback`,
//! the toolchains it went back to match their receipts byte for byte. A tripped wire says so,
//! leaves a report of kind `tripwire` in the ashtray, and sends nothing.
//!
//! The mismatch is planted with a fault point (`CIGI_FAULT=tripwire-index:1` edits the index after
//! the commit, `tripwire-toolchain:1` changes a file of the rolled-back toolchain), so the flow
//! itself is the real one.

mod common;

use std::path::PathBuf;
use std::process::Output;

use cigi_testkit::Fixture;
use common::{cigi, fixture, ok, text};
use serde_json::Value;

fn two() -> Fixture {
    let mut fx = fixture();
    fx.product("cig", "1.20.0").dry_run_output("plan: 1 op");
    fx.product("cig", "1.21.0").dry_run_output("plan: 1 op");
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.17.0")]);
    fx.carton("0.1.1", &[("cig", "1.21.0"), ("cigo", "1.17.0")]);
    fx.publish("stable");
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    fx
}

/// `args` with a fault planted and every send refused, so a leak would show as a failure.
fn planted(fx: &Fixture, fault: &str, args: &[&str]) -> Output {
    fx.cigi()
        .env("CIGI_ALLOW_SINGLE_SOURCE", "true")
        .env("CIGI_FAULT", format!("{fault}:1"))
        .env("CIG_REPORTS", "always")
        .arg("--yes")
        .args(args)
        .output()
        .expect("run ciginstaller")
}

fn reports(fx: &Fixture) -> Vec<Value> {
    let dir: PathBuf = fx.cigscript().join("ashtray");
    let mut v: Vec<Value> = std::fs::read_dir(dir)
        .map(|d| {
            d.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "json"))
                .map(|p| serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap())
                .collect()
        })
        .unwrap_or_default();
    v.sort_by_key(|r| r["id"].as_str().unwrap_or("").to_string());
    v
}

/// The one report a tripped wire left, checked for what it must be.
fn the_report(fx: &Fixture, wire: &str, severity: &str) -> Value {
    let all = reports(fx);
    assert_eq!(all.len(), 1, "one report in the ashtray: {all:?}");
    let r = all[0].clone();
    assert_eq!(r["kind"], "tripwire", "{r}");
    assert_eq!(r["origin"], "ciginstaller", "{r}");
    assert_eq!(r["details"]["wire"], wire, "{r}");
    assert_eq!(r["details"]["severity"], severity, "{r}");
    assert!(
        !r["details"]["problems"].as_array().unwrap().is_empty(),
        "{r}"
    );
    // kept at home: not sent, not filed, not asked for
    assert!(r["sent"].is_null() && r["issue"].is_null(), "{r}");
    assert_ne!(r["queue"]["requested"], true, "{r}");
    // and `crash list` shows it with the rest
    let o = fx.cigi().args(["crash", "list"]).output().unwrap();
    ok(&o);
    assert!(text(&o).contains("tripwire"), "{}", text(&o));
    r
}

#[test]
fn a6_tripwire_is_quiet_when_every_promise_holds() {
    let fx = two();
    let o = cigi(&fx, &["update", "--no-trial"]);
    ok(&o);
    assert!(!text(&o).contains("E1849"), "{}", text(&o));
    let o = cigi(&fx, &["rollback"]);
    ok(&o);
    assert!(!text(&o).contains("E1849"), "{}", text(&o));
    assert!(reports(&fx).is_empty(), "{:?}", reports(&fx));
}

#[test]
fn a6_tripwire_after_update_a_shim_that_misses_the_carton() {
    let fx = two();
    let o = planted(&fx, "tripwire-index", &["update", "--no-trial"]);
    // the update is done and stays done
    ok(&o);
    let t = text(&o);
    assert!(t.contains("updated carton 0.1.0 to 0.1.1"), "{t}");
    assert!(t.contains("E1849"), "{t}");
    assert!(t.contains("nothing was sent"), "{t}");
    assert!(t.contains("shim"), "{t}");
    let r = the_report(&fx, "update", "medium");
    assert_eq!(r["panic_location"], "tripwire:after-update");
    let st = std::fs::read_to_string(fx.cigscript().join("installer/state.json")).unwrap();
    assert!(st.contains("\"machine_carton\": \"0.1.1\""), "{st}");
}

#[test]
fn a6_tripwire_json_carries_the_wire() {
    let fx = two();
    let o = planted(&fx, "tripwire-index", &["--json", "update", "--no-trial"]);
    ok(&o);
    let line = String::from_utf8_lossy(&o.stdout)
        .lines()
        .last()
        .unwrap()
        .to_string();
    let v: Value = serde_json::from_str(&line).unwrap();
    let w = &v["data"]["tripwires"][0];
    assert_eq!(w["code"], "E1849", "{v}");
    assert_eq!(w["wire"], "update", "{v}");
    assert!(w["report"].is_string(), "{v}");
}

#[test]
fn a6_tripwire_after_rollback_the_index_and_the_files() {
    // the index
    let fx = two();
    ok(&cigi(&fx, &["update", "--no-trial"]));
    let o = planted(&fx, "tripwire-index", &["rollback"]);
    ok(&o);
    assert!(text(&o).contains("E1849"), "{}", text(&o));
    the_report(&fx, "rollback", "high");

    // a file of the toolchain it went back to
    let fx = two();
    ok(&cigi(&fx, &["update", "--no-trial"]));
    let o = planted(&fx, "tripwire-toolchain", &["rollback"]);
    ok(&o);
    let t = text(&o);
    assert!(t.contains("E1849"), "{t}");
    assert!(t.contains("differs from its receipt"), "{t}");
    the_report(&fx, "rollback", "high");

    // one tool rolled back on its own
    let fx = two();
    ok(&cigi(&fx, &["update", "--no-trial"]));
    let o = planted(&fx, "tripwire-toolchain", &["rollback", "cig"]);
    ok(&o);
    assert!(text(&o).contains("E1849"), "{}", text(&o));
    the_report(&fx, "rollback", "high");
}

/// The installer's row of the plan's `tripwires_per_product`: each wire fires on a planted fault.
#[test]
fn tripwires_per_product() {
    let fx = two();
    let o = planted(&fx, "tripwire-index", &["update", "--no-trial"]);
    ok(&o);
    assert!(text(&o).contains("E1849"), "update wire: {}", text(&o));
    let o = planted(&fx, "tripwire-toolchain", &["rollback"]);
    ok(&o);
    assert!(text(&o).contains("E1849"), "rollback wire: {}", text(&o));
    let all = reports(&fx);
    assert_eq!(all.len(), 2, "{all:?}");
    assert!(all
        .iter()
        .all(|r| r["kind"] == "tripwire" && r["origin"] == "ciginstaller"));
    let wires: Vec<&str> = all
        .iter()
        .map(|r| r["details"]["wire"].as_str().unwrap())
        .collect();
    assert!(
        wires.contains(&"update") && wires.contains(&"rollback"),
        "{wires:?}"
    );
}
