// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0
//! Review of PR #39 (SEC-004 finding 7): a journaled remove touches only paths inside the home,
//! and a toolchain path is made only from a name that passes the tool-name rule.

mod common;
use cigi_home::toolchain::{remove_toolchain, remove_toolchain_now};
use cigi_home::Journal;
use common::*;
use std::fs;

#[test]
fn txn_delete_refuses_paths_outside_the_home_and_dotdot() {
    let fx = fx();
    let outside = fx.tmp.path().join("precious");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("x"), "keep").unwrap();
    let inside = fx.home.root().join("toolchains").join("a");
    fs::create_dir_all(&inside).unwrap();

    let mut txn = fx.txn("refusals");
    for bad in [
        outside.clone(),
        outside.join("x"),
        fx.home
            .root()
            .join("toolchains")
            .join("..")
            .join("..")
            .join("precious"),
        fx.home.root().join("..").join("precious"),
        fx.home.root(),
        std::path::PathBuf::from("/etc/passwd"),
    ] {
        assert!(txn.delete(&bad).is_err(), "{} was accepted", bad.display());
    }
    // nothing changed
    assert_eq!(fs::read_to_string(outside.join("x")).unwrap(), "keep");
    assert!(inside.is_dir());
    // a path inside the home still goes
    txn.delete(&inside).unwrap();
    assert!(!inside.exists());
    txn.rollback().unwrap();
    assert!(inside.is_dir());
}

#[test]
fn toolchain_removal_takes_only_plain_names() {
    let fx = fx();
    let victim = fx.tmp.path().join("u").join("victim");
    fs::create_dir_all(victim.join("1.0.0")).unwrap();
    let mut txn = fx.txn("names");
    for tool in ["../../victim", "..", "a/b", "Cig", "", "cig/../../victim"] {
        assert!(
            remove_toolchain(&mut txn, &fx.home, tool, &v("1.0.0")).is_err(),
            "{tool:?}"
        );
        assert!(
            remove_toolchain_now(&fx.home, tool, &v("1.0.0")).is_err(),
            "{tool:?}"
        );
    }
    assert!(victim.join("1.0.0").is_dir());
    // a real one goes, both ways
    for (tool, ver) in [("cig", "1.0.0"), ("cigo", "2.0.0")] {
        fs::create_dir_all(fx.home.toolchain(tool, &v(ver))).unwrap();
    }
    remove_toolchain(&mut txn, &fx.home, "cig", &v("1.0.0")).unwrap();
    assert!(!fx.home.toolchain("cig", &v("1.0.0")).exists());
    remove_toolchain_now(&fx.home, "cigo", &v("2.0.0")).unwrap();
    assert!(!fx.home.toolchain("cigo", &v("2.0.0")).exists());
    // again: nothing to do, no error
    remove_toolchain_now(&fx.home, "cigo", &v("2.0.0")).unwrap();
    txn.rollback().unwrap();
}
