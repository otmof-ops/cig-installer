// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0

//! Tests that need a second process: lock contention and real crashes.

mod common;

use std::fs;
use std::process::Command;
use std::time::{Duration, Instant};

use cigi_home::*;
use common::*;

fn run_child(test: &str, envs: &[(&str, String)]) -> (bool, String, Option<i32>) {
    let exe = std::env::current_exe().unwrap();
    let mut c = Command::new(exe);
    c.args(["--exact", test, "--nocapture", "--test-threads=1"]);
    c.env_remove("CIGI_FAULT");
    for (k, v) in envs {
        c.env(k, v);
    }
    let out = c.output().unwrap();
    (
        out.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
        out.status.code(),
    )
}

// ------------------------------------------------------------------- lock

/// Runs only inside a child spawned by the tests below.
#[test]
fn lock_child() {
    let Ok(root) = std::env::var("CIGI_TEST_LOCK_HOME") else {
        return;
    };
    let h = Home::at(root.into());
    let mode = std::env::var("CIGI_TEST_LOCK_MODE").unwrap_or_default();
    let wait = if mode == "wait" {
        Duration::from_secs(10)
    } else {
        Duration::from_millis(300)
    };
    let started = Instant::now();
    let mut waited_on = None;
    match h.lock_with(wait, &mut |pid| waited_on = Some(pid)) {
        Ok(_l) => println!(
            "ACQUIRED after_ms={} waited_on={waited_on:?}",
            started.elapsed().as_millis()
        ),
        Err(e) => println!(
            "FAILED {} after_ms={} waited_on={waited_on:?}",
            e.code(),
            started.elapsed().as_millis()
        ),
    }
}

#[test]
fn a_second_process_waits_then_gets_e1834() {
    let fx = fx(); // holds the lock in this process
    let (ok, out, _) = run_child(
        "lock_child",
        &[("CIGI_TEST_LOCK_HOME", fx.home.root().display().to_string())],
    );
    assert!(ok, "{out}");
    assert!(out.contains("FAILED E1834"), "{out}");
    assert!(
        out.contains(&format!("waited_on=Some(Some({}))", std::process::id())),
        "callback got our pid: {out}"
    );
    let ms: u128 = out
        .split("after_ms=")
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert!(ms >= 300, "it waited the whole budget: {ms} ms");
}

#[test]
fn a_waiting_process_gets_the_lock_when_it_is_released() {
    let fx = fx();
    let root = fx.home.root().display().to_string();
    let Fx {
        tmp: _tmp,
        home: _home,
        lock,
    } = fx;
    let child = std::thread::spawn(move || {
        run_child(
            "lock_child",
            &[
                ("CIGI_TEST_LOCK_HOME", root),
                ("CIGI_TEST_LOCK_MODE", "wait".to_string()),
            ],
        )
    });
    std::thread::sleep(Duration::from_millis(400));
    drop(lock);
    let (ok, out, _) = child.join().unwrap();
    assert!(ok, "{out}");
    assert!(out.contains("ACQUIRED"), "{out}");
    assert!(out.contains("waited_on=Some("), "it did wait first: {out}");
}

// ------------------------------------------------- real crashes (abort())

/// The transaction a crashing child runs: an update that adds a toolchain,
/// writes state and flips the index. Runs only inside a child.
#[test]
fn crash_child() {
    let Ok(root) = std::env::var("CIGI_TEST_CRASH_HOME") else {
        return;
    };
    let h = Home::at(root.clone().into());
    let l = h.lock().unwrap();
    let stage = std::path::Path::new(&root)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("stage");
    let files: Vec<StagedFile> = [
        ("bin/cigo", true),
        ("orchestrate.cig", false),
        ("docs/a.md", false),
    ]
    .iter()
    .map(|(rel, exec)| StagedFile {
        path: (*rel).into(),
        source: stage.join(rel.replace('/', "_")),
        exec: *exec,
    })
    .collect();
    let mut t = Txn::begin(&h, "update", &l).unwrap();
    materialize(&mut t, &h, "cigo", &v("1.17.0"), &files, prov()).unwrap();
    let mut s = State::load(&h).unwrap();
    s.machine_carton = Some("0.2.0".into());
    s.save_in(&mut t).unwrap();
    let idx = ShimIndex {
        machine: Some("0.2.0".into()),
        ..ShimIndex::default()
    };
    idx.flip_in(&mut t).unwrap();
    t.commit().unwrap();
    println!("FINISHED");
}

fn crash_setup() -> (Fx, Vec<String>, String) {
    let fx = fx();
    for (name, body) in [
        ("bin_cigo", "#!/bin/sh\n"),
        ("orchestrate.cig", "orch"),
        ("docs_a.md", "doc"),
    ] {
        fx.src(name, body.as_bytes());
    }
    // a committed baseline
    let mut t = fx.txn("baseline");
    let s = State {
        machine_carton: Some("0.1.0".into()),
        ..State::default()
    };
    s.save_in(&mut t).unwrap();
    let idx = ShimIndex {
        machine: Some("0.1.0".into()),
        ..ShimIndex::default()
    };
    let old_index = idx.render();
    idx.flip_in(&mut t).unwrap();
    t.commit().unwrap();
    let snap = snapshot(&fx.home.root());
    (fx, snap, old_index)
}

fn crash_at(fault: &str) -> (Fx, Vec<String>, String, (bool, String, Option<i32>)) {
    let (fx, snap, old_index) = crash_setup();
    let root = fx.home.root().display().to_string();
    // the child must be able to take the lock
    let Fx { tmp, home, lock } = fx;
    drop(lock);
    let res = run_child(
        "crash_child",
        &[
            ("CIGI_TEST_CRASH_HOME", root),
            ("CIGI_FAULT", fault.to_string()),
        ],
    );
    let lock = home.lock().unwrap();
    (Fx { tmp, home, lock }, snap, old_index, res)
}

#[test]
fn killed_before_the_flip_is_rolled_back_to_the_byte() {
    for fault in [
        "mkdir:1", "mkdir:2", "store:1", "store:3", "link:1", "link:3", "move:1", "write:1",
        "flip:1",
    ] {
        let (fx, snap, old_index, (ok, out, code)) = crash_at(fault);
        assert!(
            !ok && !out.contains("FINISHED") && !out.contains("panicked"),
            "{fault}: the child must have been aborted: {out} {code:?}"
        );
        let r = recover(&fx.home, &fx.lock).unwrap();
        assert_eq!(r.len(), 1, "{fault}");
        assert!(
            matches!(r[0].outcome, RecoverOutcome::RolledBack { .. }),
            "{fault}: {:?}",
            r[0]
        );
        assert_eq!(
            snapshot(&fx.home.root()),
            snap,
            "{fault}: home is as before"
        );
        assert_eq!(
            fs::read_to_string(fx.home.shim_index_path()).unwrap(),
            old_index,
            "{fault}"
        );
        assert!(installed(&fx.home).is_empty(), "{fault}");
        assert!(
            recover(&fx.home, &fx.lock).unwrap().is_empty(),
            "{fault}: recovery is idempotent"
        );
    }
}

#[test]
fn killed_after_the_rename_is_rolled_forward() {
    for fault in ["flipped:1", "flip:2"] {
        let (fx, _snap, _old, (ok, out, _)) = crash_at(fault);
        if fault == "flip:2" {
            // only one flip line exists: the fault never fires and the child finishes
            assert!(ok && out.contains("FINISHED"), "{out}");
            assert!(recover(&fx.home, &fx.lock).unwrap().is_empty());
            continue;
        }
        assert!(!ok && !out.contains("panicked"), "{out}");
        let r = recover(&fx.home, &fx.lock).unwrap();
        assert_eq!(r[0].outcome, RecoverOutcome::RolledForward, "{fault}");
        assert_eq!(
            State::load(&fx.home).unwrap().machine_carton.as_deref(),
            Some("0.2.0")
        );
        assert!(fs::read_to_string(fx.home.shim_index_path())
            .unwrap()
            .contains("machine\t0.2.0"));
        assert_eq!(installed(&fx.home).len(), 1);
        assert!(verify_toolchain(&fx.home, "cigo", &v("1.17.0")).is_empty());
    }
}

#[test]
fn killed_after_the_commit_line_has_nothing_to_recover() {
    let (fx, _snap, _old, (ok, out, _)) = crash_at("commit:1");
    assert!(!ok && !out.contains("panicked"), "{out}");
    assert!(recover(&fx.home, &fx.lock).unwrap().is_empty());
    assert_eq!(
        State::load(&fx.home).unwrap().machine_carton.as_deref(),
        Some("0.2.0")
    );
}

#[test]
fn an_uninterrupted_child_commits() {
    let (fx, _snap, _old, (ok, out, _)) = crash_at("none:1");
    assert!(ok && out.contains("FINISHED"), "{out}");
    assert!(recover(&fx.home, &fx.lock).unwrap().is_empty());
    assert_eq!(installed(&fx.home).len(), 1);
}
