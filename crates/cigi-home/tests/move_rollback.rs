// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! A rolled-back `Txn::move_path` puts the moved file (or directory) back, byte for byte: in
//! process, and with the process killed at every point around the move.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use cigi_home::*;
use common::*;

fn data(root: &Path) -> PathBuf {
    root.parent().unwrap().join("data")
}

fn body() -> Vec<u8> {
    (0..=255u8).cycle().take(70_000).collect()
}

fn make_data(root: &Path, readonly: bool) {
    let d = data(root);
    fs::create_dir_all(d.join("dir")).unwrap();
    fs::write(d.join("a.bin"), body()).unwrap();
    fs::write(d.join("dir").join("f"), b"inner").unwrap();
    if readonly {
        platform::set_mode(&d.join("a.bin"), 0o444).unwrap();
    }
}

/// Where the moved file is, and that it is whole.
fn located(root: &Path) -> (bool, bool) {
    let d = data(root);
    let whole = |p: PathBuf| fs::read(&p).is_ok_and(|b| b == body());
    (whole(d.join("a.bin")), whole(d.join("b.bin")))
}

#[test]
fn a_rolled_back_move_puts_a_file_back_byte_for_byte() {
    let fx = fx();
    let root = fx.home.root();
    make_data(&root, false);
    let d = data(&root);
    let mut t = fx.txn("move");
    t.move_path(&d.join("a.bin"), &d.join("b.bin")).unwrap();
    assert_eq!(located(&root), (false, true));
    let rep = t.rollback().unwrap();
    assert!(rep.is_complete(), "{:?}", rep.failed);
    assert_eq!(located(&root), (true, false));
}

#[test]
fn a_rolled_back_move_puts_a_directory_back_whole() {
    let fx = fx();
    let root = fx.home.root();
    make_data(&root, false);
    let d = data(&root);
    let mut t = fx.txn("move");
    t.move_path(&d.join("dir"), &d.join("moved")).unwrap();
    assert!(!d.join("dir").exists());
    t.rollback().unwrap();
    assert_eq!(fs::read(d.join("dir").join("f")).unwrap(), b"inner");
    assert!(!d.join("moved").exists());
}

#[test]
fn a_rollback_run_twice_after_a_move_changes_nothing_more() {
    let fx = fx();
    let root = fx.home.root();
    make_data(&root, false);
    let d = data(&root);
    let mut t = fx.txn("move");
    t.move_path(&d.join("a.bin"), &d.join("b.bin")).unwrap();
    drop(t); // as a crash: recover settles it
    for _ in 0..2 {
        recover(&fx.home, &fx.lock).unwrap();
        assert_eq!(located(&root), (true, false));
    }
}

/// Runs only inside a child: moves the file, then stops where `CIGI_FAULT` says.
#[test]
fn move_child() {
    let Ok(root) = std::env::var("CIGI_TEST_MOVE_HOME") else {
        return;
    };
    let h = Home::at(root.clone().into());
    let l = h.lock().unwrap();
    let d = data(Path::new(&root));
    let mut t = Txn::begin(&h, "move", &l).unwrap();
    t.move_path(&d.join("a.bin"), &d.join("b.bin")).unwrap();
    journal::fault_after("after-move");
    t.commit().unwrap();
    println!("FINISHED");
}

fn kill_sweep(readonly: bool) {
    for fault in ["begin:1", "move:1", "after-move:1", "commit:1", "none:1"] {
        let fx = fx();
        let root = fx.home.root();
        make_data(&root, readonly);
        let Fx { tmp, home, lock } = fx;
        drop(lock);
        let out = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "move_child", "--nocapture", "--test-threads=1"])
            .env("CIGI_TEST_MOVE_HOME", root.display().to_string())
            .env("CIGI_FAULT", fault)
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert_eq!(out.status.success(), fault == "none:1", "{fault}: {text}");
        let lock = home.lock().unwrap();
        let rec = recover(&home, &lock).unwrap();
        let loc = located(&root);
        match fault {
            // killed before the commit line: rolled back, the file is where it began
            "begin:1" | "move:1" | "after-move:1" => {
                assert_eq!(rec.len(), 1, "{fault}");
                assert!(
                    matches!(rec[0].outcome, RecoverOutcome::RolledBack { .. }),
                    "{fault}: {:?}",
                    rec[0]
                );
                assert_eq!(loc, (true, false), "{fault}");
            }
            // committed: nothing to recover, the file is where the move put it
            _ => {
                assert!(rec.is_empty(), "{fault}");
                assert_eq!(loc, (false, true), "{fault}");
            }
        }
        assert!(recover(&home, &lock).unwrap().is_empty(), "{fault}");
        drop(tmp);
    }
}

#[test]
fn killed_around_a_file_move_the_file_is_whole_in_one_place() {
    kill_sweep(false);
}

#[cfg(windows)]
#[test]
fn windows_killed_around_a_read_only_file_move_the_file_is_whole_in_one_place() {
    kill_sweep(true);
}
