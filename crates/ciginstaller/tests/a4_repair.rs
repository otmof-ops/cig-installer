// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! Phase 4 acceptance (SPEC 14.3): `repair`. Files come back byte for byte from the store, or
//! from a download that is admitted again, or `E1860` leaves the home as it was.

// Unix only: repair tests damage files through Unix modes and run shell-script fakes.
// The Windows runtime drafts add their own equivalents.
#![cfg(unix)]

mod common;

use std::path::Path;

use cigi_testkit::{Fixture, Snapshot};
use common::{cigi, fixture, ok, text};

fn setup() -> Fixture {
    let mut fx = fixture();
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.16.0")]);
    fx.publish("stable");
    ok(&cigi(&fx, &["install", "carton"]));
    fx
}

/// What repair puts right: the files, the shims, the store, the humidor, and the two files of
/// state that do not change.
fn view(fx: &Fixture) -> Snapshot {
    let root = fx.cigscript();
    let mut s = Snapshot::default();
    for p in ["bin", "toolchains", "store/objects", "humidor"] {
        for (k, v) in Snapshot::take(&root.join(p)).entries {
            s.entries.insert(format!("{p}/{k}"), v);
        }
    }
    for (k, v) in Snapshot::take(&root).entries {
        if k == "installer/shim-index" {
            s.entries.insert(k, v);
        }
    }
    s
}

fn tc(fx: &Fixture, rest: &str) -> std::path::PathBuf {
    fx.cigscript().join("toolchains").join(rest)
}

/// What a tool does that breaks one link only: write a new file and rename it over the old.
fn replace_by_rename(p: &Path, bytes: &[u8]) {
    let tmp = p.with_extension("new");
    std::fs::write(&tmp, bytes).unwrap();
    std::fs::rename(&tmp, p).unwrap();
}

/// What a tool does that damages the shared object: open the read-only file and write in place.
fn write_in_place(p: &Path, bytes: &[u8]) {
    std::fs::set_permissions(p, std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap();
    std::fs::write(p, bytes).unwrap();
}

fn damage_three(fx: &Fixture) {
    std::fs::remove_file(tc(fx, "cigo/1.16.0/orchestrate.cig")).unwrap(); // missing
    replace_by_rename(
        &tc(fx, "cigo/1.16.0/bin/cigo"),
        b"#!/bin/sh\necho tampered\n",
    ); // changed
    std::fs::write(tc(fx, "cig/1.20.0/bin/stray"), "extra").unwrap(); // extra
}

fn files(o: &std::process::Output) -> serde_json::Value {
    let line = String::from_utf8_lossy(&o.stdout)
        .lines()
        .last()
        .unwrap_or("")
        .to_string();
    serde_json::from_str::<serde_json::Value>(&line).unwrap_or_default()["data"].clone()
}

#[test]
fn a4_repair_restores_from_the_store_byte_for_byte() {
    let fx = setup();
    let whole = view(&fx);
    damage_three(&fx);
    assert!(!view(&fx).diff(&whole).is_empty());
    let o = cigi(&fx, &["--json", "repair"]);
    ok(&o);
    let d = files(&o);
    assert_eq!(d["restored"].as_array().unwrap().len(), 2, "{d:#}");
    assert!(d["restored"]
        .as_array()
        .unwrap()
        .iter()
        .all(|r| r["from"] == "from the store"));
    assert_eq!(d["removed"].as_array().unwrap().len(), 1);
    view(&fx).assert_same(&whole);
    // restored files are links of the store objects again (they share an inode)
    use cigi_home::platform::ino;
    let obj = std::fs::read_dir(fx.cigscript().join("store/objects"))
        .unwrap()
        .flatten()
        .flat_map(|d| std::fs::read_dir(d.path()).unwrap().flatten())
        .map(|e| ino(&e.path()))
        .collect::<Vec<_>>();
    assert!(obj.contains(&ino(&tc(&fx, "cigo/1.16.0/bin/cigo"))));
    // the tools run
    let o = fx.run_shim("cigo", &fx.project, &[]);
    ok(&o);
    assert_eq!(
        String::from_utf8_lossy(&o.stdout).trim(),
        "cigo 1.16.0 uses cig 1.20.0"
    );
    // a second repair has nothing to do and writes no transaction
    let txns = || {
        std::fs::read_dir(fx.cigscript().join("installer/txn"))
            .unwrap()
            .count()
    };
    let before = txns();
    let o = cigi(&fx, &["--json", "repair"]);
    ok(&o);
    assert_eq!(files(&o)["changed"], false);
    assert_eq!(txns(), before);
}

#[test]
fn a4_repair_refetches_when_the_store_object_is_damaged_too() {
    let fx = setup();
    let whole = view(&fx);
    // written in place: the toolchain file, the store object and the humidor link are one file
    write_in_place(
        &tc(&fx, "cigo/1.16.0/orchestrate.cig"),
        b"# changed in place\n",
    );
    write_in_place(&tc(&fx, "cig/1.20.0/bin/cig"), b"#!/bin/sh\necho broken\n");
    let srv = fx.serve();
    let o = fx
        .cigi_with_mirror(&srv.url())
        .env("CIGI_ALLOW_SINGLE_SOURCE", "true")
        .args(["--yes", "--json", "repair"])
        .output()
        .unwrap();
    ok(&o);
    let d = files(&o);
    assert!(
        d["restored"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["from"] == "from a fresh download"),
        "{d:#}"
    );
    let paths = srv.paths();
    assert!(
        paths.iter().any(|p| p.contains("cig-orchestrator-v1.16.0")),
        "{paths:?}"
    );
    assert!(paths.iter().any(|p| p.contains("cig-v1.20.0")), "{paths:?}");
    view(&fx).assert_same(&whole);
    // the humidor link is the repaired binary again
    let a = std::fs::read(fx.cigscript().join("humidor/1.20.0/cig")).unwrap();
    let b = std::fs::read(tc(&fx, "cig/1.20.0/bin/cig")).unwrap();
    assert_eq!(a, b);
    let o = cigi(&fx, &["--json", "doctor"]);
    ok(&o);
}

#[test]
fn a4_repair_with_the_server_down_is_e1860_and_changes_nothing() {
    let fx = setup();
    write_in_place(
        &tc(&fx, "cigo/1.16.0/orchestrate.cig"),
        b"# changed in place\n",
    );
    // the lock file holds a pid, which changes with every command: not part of "the home"
    let snap = |fx: &Fixture| {
        Snapshot::take(&fx.user_home).without(&[
            ".cigscript/installer/lock",
            ".cigscript/installer.lock",
            ".cigscript/installer/lock.pid",
        ])
    };
    // the server goes away: the releases folder is gone, only the cached manifest is left
    std::fs::remove_dir_all(&fx.releases).unwrap();
    let before = snap(&fx);
    let o = cigi(&fx, &["repair"]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(text(&o).contains("E1860"), "{}", text(&o));
    snap(&fx).assert_same(&before);
    // offline says the same
    let o = cigi(&fx, &["--offline", "repair"]);
    assert!(text(&o).contains("E1860"), "{}", text(&o));
    snap(&fx).assert_same(&before);
}

#[test]
fn a4_repair_one_tool_only() {
    let fx = setup();
    damage_three(&fx);
    ok(&cigi(&fx, &["repair", "--tool", "cigo"]));
    // cigo is whole, cig keeps its stray file
    assert!(tc(&fx, "cigo/1.16.0/orchestrate.cig").exists());
    assert!(tc(&fx, "cig/1.20.0/bin/stray").exists());
    let o = cigi(&fx, &["repair", "--tool", "nothing"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(text(&o).contains("E1863"), "{}", text(&o));
    ok(&cigi(&fx, &["repair"]));
    assert!(!tc(&fx, "cig/1.20.0/bin/stray").exists());
}

#[test]
fn a4_repair_regenerates_shims_and_index() {
    let fx = setup();
    let whole = view(&fx);
    for n in ["cig", "cigo", "cigi", "ciginstaller"] {
        std::fs::remove_file(fx.cigscript().join("bin").join(n)).unwrap();
    }
    std::fs::remove_file(fx.cigscript().join("installer/shim-index")).unwrap();
    ok(&cigi(&fx, &["repair"]));
    view(&fx).assert_same(&whole);
    ok(&fx.run_shim("cig", &fx.project, &["--version"]));
}

#[test]
fn a4_repair_dry_run_writes_nothing() {
    let fx = setup();
    damage_three(&fx);
    // the lock file holds a pid: a dry run that took the lock would change it
    let before = Snapshot::take(&fx.user_home);
    let o = cigi(&fx, &["--json", "repair", "--dry-run"]);
    ok(&o);
    assert_eq!(files(&o)["dry_run"], true);
    Snapshot::take(&fx.user_home).assert_same(&before);
}

#[test]
fn a4_repair_kill_sweep() {
    let mut crashes = 0;
    let (mut pre_seen, mut post_seen) = (0, 0);
    for phase in [
        "mkdir", "store", "link", "delete", "write", "flip", "flipped", "commit",
    ] {
        for n in 1..40 {
            let fx = setup();
            let whole = view(&fx);
            write_in_place(&tc(&fx, "cigo/1.16.0/orchestrate.cig"), b"# in place\n");
            replace_by_rename(&tc(&fx, "cig/1.20.0/bin/cig"), b"#!/bin/sh\necho x\n");
            std::fs::write(tc(&fx, "cig/1.20.0/bin/stray"), "extra").unwrap();
            std::fs::remove_file(fx.cigscript().join("bin/cigo")).unwrap();
            let damaged = view(&fx);
            let o = fx
                .cigi()
                .env("CIGI_ALLOW_SINGLE_SOURCE", "true")
                .env("CIGI_FAULT", format!("{phase}:{n}"))
                .args(["--yes", "repair"])
                .output()
                .unwrap();
            if o.status.code().is_some() {
                break; // this phase has fewer than n lines
            }
            crashes += 1;
            // the next command that takes the lock settles the journal (a dry run of `repair` does not)
            let o = cigi(&fx, &["gc", "--dry-run"]);
            ok(&o);
            let now = view(&fx);
            if now.diff(&whole).is_empty() {
                post_seen += 1;
            } else if now.diff(&damaged).is_empty() {
                pre_seen += 1;
            } else {
                panic!(
                    "{phase}:{n}: neither damaged nor whole: {:?} / {:?}",
                    now.diff(&damaged),
                    now.diff(&whole)
                );
            }
            // and a repair after any kill finishes the job
            ok(&cigi(&fx, &["repair"]));
            view(&fx).assert_same(&whole);
        }
    }
    assert!(crashes >= 8, "only {crashes} fault points reached");
    assert!(
        pre_seen > 0 && post_seen > 0,
        "pre {pre_seen}, post {post_seen}"
    );
}
