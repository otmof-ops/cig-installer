// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0
//! `update` run from the managed binary (`bin/ciginstaller`) with a newer installer on offer: only
//! a real update replaces the installer; a dry run and a trial change nothing (SPEC 10, 12.2), a
//! failed self-check is a note, and the re-exec mark counts only for the version it names.
#![cfg(unix)]

mod common;
use std::os::unix::fs::MetadataExt;
use std::process::{Command, Output};

use cigi_testkit::{Fixture, Snapshot};
use common::*;

fn setup(selfcheck_fails: bool) -> Fixture {
    let mut fx = fixture();
    let p = fx.product("ciginstaller", "0.1.1");
    if selfcheck_fails {
        p.selfcheck_fails();
    }
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.16.0")]);
    fx.carton(
        "0.1.1",
        &[
            ("cig", "1.20.0"),
            ("cigo", "1.16.0"),
            ("ciginstaller", "0.1.1"),
        ],
    );
    fx.publish("stable");
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    // the first good command marks the build healthy; do it now
    ok(&output(managed(&fx).args(["--yes", "list"])));
    fx
}

fn managed(fx: &Fixture) -> Command {
    let base = fx.cigi();
    let mut c = Command::new(fx.cigscript().join("bin/ciginstaller"));
    c.env_clear();
    for (k, v) in base.get_envs() {
        if let Some(v) = v {
            c.env(k, v);
        }
    }
    c.env("CIGI_ALLOW_SINGLE_SOURCE", "true")
        .current_dir(&fx.project);
    c
}

fn output(c: &mut Command) -> Output {
    let mut tries = 0;
    loop {
        match c.output() {
            Err(e) if e.raw_os_error() == Some(26) && tries < 200 => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            r => return r.expect("run the installer"),
        }
    }
}

fn state(fx: &Fixture) -> serde_json::Value {
    let t = std::fs::read_to_string(fx.cigscript().join("installer/state.json")).unwrap();
    serde_json::from_str(&t).unwrap()
}

fn ino(fx: &Fixture) -> u64 {
    std::fs::metadata(fx.cigscript().join("bin/ciginstaller"))
        .unwrap()
        .ino()
}

fn view(fx: &Fixture) -> Snapshot {
    let root = fx.cigscript();
    let mut s = Snapshot::default();
    for p in ["bin", "toolchains", "installer/previous"] {
        for (k, v) in Snapshot::take(&root.join(p)).entries {
            s.entries.insert(format!("{p}/{k}"), v);
        }
    }
    for (k, v) in Snapshot::take(&root.join("installer")).entries {
        if k == "state.json" || k == "shim-index" {
            s.entries.insert(format!("installer/{k}"), v);
        }
    }
    s
}

#[test]
fn a3_dry_run_and_trial_only_never_self_update() {
    let fx = setup(false);
    let (before, inode) = (view(&fx), ino(&fx));
    for flag in ["--dry-run", "--trial-only"] {
        let o = output(managed(&fx).args(["--yes", "update", flag]));
        ok(&o);
        assert!(
            !text(&o).contains("ciginstaller updated"),
            "{flag}: {}",
            text(&o)
        );
        assert_eq!(ino(&fx), inode, "{flag}: bin/ciginstaller was replaced");
        assert_eq!(state(&fx)["installer"]["version"], "0.1.0", "{flag}");
        view(&fx).assert_same(&before);
    }
}

#[test]
fn a3_a_real_update_self_updates_and_the_mark_counts_only_for_its_version() {
    // a stale or planted mark disables nothing
    let fx = setup(false);
    let inode = ino(&fx);
    let o = output(
        managed(&fx)
            .env("CIGI_SELF_UPDATED", "9.9.9")
            .args(["--yes", "update"]),
    );
    ok(&o);
    assert!(text(&o).contains("updated 0.1.0 -> 0.1.1"), "{}", text(&o));
    assert_ne!(ino(&fx), inode);
    // a mark naming the running version (what a re-exec sets) skips the step
    let fx = setup(false);
    let inode = ino(&fx);
    let o = output(
        managed(&fx)
            .env("CIGI_SELF_UPDATED", "0.1.0")
            .args(["--yes", "update"]),
    );
    ok(&o);
    assert_eq!(ino(&fx), inode);
    assert_eq!(state(&fx)["machine_carton"], "0.1.1");
}

#[test]
fn a3_a_failed_self_check_is_a_note_and_the_cartons_still_update() {
    let fx = setup(true);
    let inode = ino(&fx);
    let o = output(managed(&fx).args(["--yes", "update"]));
    ok(&o);
    assert!(text(&o).contains("E1853"), "{}", text(&o));
    assert_eq!(ino(&fx), inode, "the old binary is back");
    assert_eq!(state(&fx)["machine_carton"], "0.1.1");
}
