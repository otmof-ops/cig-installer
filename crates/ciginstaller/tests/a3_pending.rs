// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0
//! The deletes that follow a commit (a rollback that undoes an update, `gc`): recorded in the
//! change's own transaction, never seen by a shim (the index stops naming them first), finished by
//! the next `gc` after a crash between the commit and the deletes, and cleared once done.

mod common;

use std::process::Output;

use cigi_testkit::Fixture;
use common::{cigi, fixture, ok, text};

fn state(fx: &Fixture) -> serde_json::Value {
    serde_json::from_str(
        &std::fs::read_to_string(fx.cigscript().join("installer/state.json")).unwrap(),
    )
    .unwrap()
}

fn index(fx: &Fixture) -> String {
    std::fs::read_to_string(fx.cigscript().join("installer/shim-index")).unwrap()
}

/// The installer with a fault point armed: the process stops there, as a crash would.
fn faulted(fx: &Fixture, point: &str, args: &[&str]) -> Output {
    fx.cigi()
        .env("CIGI_ALLOW_SINGLE_SOURCE", "true")
        .env("CIGI_FAULT", format!("{point}:1"))
        .arg("--yes")
        .args(args)
        .output()
        .expect("run ciginstaller")
}

fn pending(fx: &Fixture) -> Vec<String> {
    state(fx)["pending_deletes"]
        .as_array()
        .map(|a| a.iter().map(|e| e.to_string()).collect())
        .unwrap_or_default()
}

#[test]
fn a3_a_crash_after_the_undo_commits_is_finished_by_gc() {
    let mut fx = fixture();
    fx.carton("0.1.0", &[("cig", "1.20.0")]);
    fx.publish("stable");
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    fx.carton("0.1.1", &[("cig", "1.21.0")]);
    fx.publish("stable");
    ok(&cigi(&fx, &["update", "--no-trial"]));
    let doomed = fx.cigscript().join("toolchains/cig/1.21.0");
    assert!(doomed.is_dir());

    // the rollback undoes the update, and stops between its commit and its deletes
    let o = faulted(&fx, "pending-delete", &["rollback"]);
    assert_ne!(o.status.code(), Some(0), "{}", text(&o));
    assert_eq!(state(&fx)["machine_carton"], "0.1.0");
    // the index already names nothing it is about to delete; the delete is recorded
    assert!(!index(&fx).contains("1.21.0"), "{}", index(&fx));
    assert!(doomed.is_dir());
    assert_eq!(pending(&fx), vec![r#"["cig","1.21.0"]"#.to_string()]);

    // the next gc finishes it, whatever its age, and the record is cleared
    ok(&cigi(&fx, &["gc"]));
    assert!(!doomed.exists());
    assert!(pending(&fx).is_empty(), "{:?}", pending(&fx));
}

#[test]
fn a3_a_rollback_that_finishes_leaves_no_record() {
    let mut fx = fixture();
    fx.carton("0.1.0", &[("cig", "1.20.0")]);
    fx.publish("stable");
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    fx.carton("0.1.1", &[("cig", "1.21.0")]);
    fx.publish("stable");
    ok(&cigi(&fx, &["update", "--no-trial"]));
    ok(&cigi(&fx, &["rollback"]));
    assert!(!fx.cigscript().join("toolchains/cig/1.21.0").exists());
    assert!(
        state(&fx).get("pending_deletes").is_none(),
        "{}",
        state(&fx)
    );
}
