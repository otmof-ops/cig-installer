// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! The Windows self-update's pieces (issue #8). The ones named `windows_*` run only on the
//! Windows CI job; the rest are about shared logic and run everywhere.

use std::fs;
use std::path::Path;
use std::time::Duration;

use cigi_home::{Home, Journal};

use super::*;

fn home() -> (tempfile::TempDir, Home) {
    let t = tempfile::tempdir().unwrap();
    let h = Home::at(t.path().join(".cigscript"));
    h.ensure_layout().unwrap();
    fs::create_dir_all(h.installer().join("previous")).unwrap();
    (t, h)
}

fn text(p: &Path) -> Option<String> {
    fs::read_to_string(p).ok()
}

#[test]
fn sweep_removes_asides_and_leaves_the_kept_installer() {
    let (_t, h) = home();
    let prev = h.installer().join("previous");
    for n in [".aside-t1-ciginstaller.exe", "ciginstaller.failed.exe"] {
        fs::write(prev.join(n), "x").unwrap();
    }
    fs::write(prev.join(exe_name()), "kept").unwrap();
    assert_eq!(sweep_aside(&h, Duration::ZERO), 0);
    assert_eq!(text(&prev.join(exe_name())).as_deref(), Some("kept"));
    assert_eq!(fs::read_dir(&prev).unwrap().count(), 1);
}

#[test]
fn sweep_of_a_home_without_a_previous_directory_is_a_no_op() {
    let t = tempfile::tempdir().unwrap();
    assert_eq!(
        sweep_aside(&Home::at(t.path().join("h")), Duration::ZERO),
        0
    );
}

#[test]
fn same_bytes_looks_at_content_not_names() {
    let t = tempfile::tempdir().unwrap();
    let (a, b, c) = (t.path().join("a"), t.path().join("b"), t.path().join("c"));
    fs::write(&a, "shim").unwrap();
    fs::write(&b, "shim").unwrap();
    fs::write(&c, "shin").unwrap();
    assert!(same_bytes(&a, &b));
    assert!(!same_bytes(&a, &c));
    assert!(!same_bytes(&a, &t.path().join("missing")));
}

#[test]
fn the_markers_live_where_the_shim_looks() {
    let (_t, h) = home();
    assert_eq!(pending_check_path(&h), h.installer().join("pending-check"));
    assert_eq!(restored_path(&h), h.installer().join("restored"));
}

#[test]
fn a_restore_brings_the_state_back_to_the_build_that_runs() {
    let v = |s: &str| Version::parse(s).unwrap();
    let mut st = State::default();
    st.installer.version = "0.1.1".into();
    st.installer.previous = Some("0.1.0".into());
    st.installer.healthy = false;
    assert!(reconcile_restored(&mut st, &v("0.1.0"), "0.1.1"));
    assert_eq!(st.installer.version, "0.1.0");
    assert_eq!(st.installer.previous, None);
    assert_eq!(st.history.last().unwrap().what, "self update restored");
    // already in line, or the running build is not the recorded previous: nothing to do
    assert!(!reconcile_restored(&mut st, &v("0.1.0"), "0.1.1"));
    st.installer.version = "0.1.1".into();
    st.installer.previous = Some("0.0.9".into());
    assert!(!reconcile_restored(&mut st, &v("0.1.0"), "0.1.1"));
}

#[test]
fn a_replace_that_is_not_refused_places_the_file_and_asides_nothing() {
    let (_t, h) = home();
    let lock = h.lock().unwrap();
    let (from, to) = (h.root().join("new.bin"), h.root().join("bin").join("x"));
    fs::create_dir_all(h.bin()).unwrap();
    fs::write(&from, "new").unwrap();
    fs::write(&to, "old").unwrap();
    let mut txn = Txn::begin(&h, "t", &lock).unwrap();
    let aside = h.installer().join("previous");
    assert!(!replace_or_aside(&mut txn, &from, &to, &aside).unwrap());
    assert_eq!(text(&to).as_deref(), Some("new"));
    txn.rollback().unwrap();
    assert_eq!(text(&to).as_deref(), Some("old"));
}

/// Windows: a program sleeps from a copy of an exe in `bin\`; the file can be renamed aside
/// while it runs, the new one goes in, and a rollback puts the old bytes back.
#[cfg(windows)]
mod on_windows {
    use super::*;
    use std::process::{Child, Command, Stdio};

    /// A copy of `cmd.exe` as `bin\ciginstaller.exe`, running a ten-second ping: a real, running image.
    fn sleeper(h: &Home) -> (std::path::PathBuf, Child) {
        fs::create_dir_all(h.bin()).unwrap();
        let exe = h.bin().join("ciginstaller.exe");
        let sys = std::env::var_os("SystemRoot")
            .map(std::path::PathBuf::from)
            .unwrap();
        fs::copy(sys.join("System32").join("cmd.exe"), &exe).unwrap();
        let child = Command::new(&exe)
            .args(["/c", "ping -n 11 127.0.0.1 > nul"])
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(300));
        (exe, child)
    }

    fn fresh(h: &Home) -> std::path::PathBuf {
        let p = h.root().join("new-installer.exe");
        fs::write(&p, "the new build").unwrap();
        p
    }

    /// The premise on a real runner: a running image cannot be written, the replace through the
    /// journal goes through anyway (POSIX rename semantics), and the old bytes stay recoverable.
    #[test]
    fn windows_a_running_exe_is_replaced_through_the_journal_and_rolled_back() {
        let (_t, h) = home();
        let lock = h.lock().unwrap();
        let (exe, mut child) = sleeper(&h);
        assert!(fs::write(&exe, "x").is_err());
        let old = fs::read(&exe).unwrap();
        let new = fresh(&h);
        let mut txn = Txn::begin(&h, "t", &lock).unwrap();
        let prev = h.installer().join("previous");
        // whichever way this Windows goes, the new file is in place afterwards
        replace_or_aside(&mut txn, &new, &exe, &prev).unwrap();
        assert_eq!(text(&exe).as_deref(), Some("the new build"));
        // a failed self-check: the journal puts the old build back
        txn.rollback().unwrap();
        assert_eq!(fs::read(&exe).unwrap(), old);
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(sweep_aside(&h, Duration::from_secs(3)), 0);
    }

    /// The aside branch itself, on a real running image: the rename works, the new file lands.
    #[test]
    fn windows_a_running_exe_renamed_aside_gets_the_new_one_placed() {
        let (_t, h) = home();
        let (exe, mut child) = sleeper(&h);
        let new = fresh(&h);
        let prev = h.installer().join("previous");
        aside_and_place("t1", &new, &exe, &prev).unwrap();
        assert_eq!(text(&exe).as_deref(), Some("the new build"));
        assert!(prev.join(".aside-t1-ciginstaller.exe").is_file());
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(sweep_aside(&h, Duration::from_secs(3)), 0);
    }

    /// A kill between the aside and the placement: the journal record made before the rename
    /// brings the old file back, with the new one absent, whichever way the replace went.
    #[test]
    fn windows_a_kill_between_the_aside_and_the_placement_is_undone_by_the_journal() {
        let (_t, h) = home();
        let lock = h.lock().unwrap();
        let (exe, mut child) = sleeper(&h);
        let old = fs::read(&exe).unwrap();
        let new = fresh(&h);
        let mut txn = Txn::begin(&h, "t", &lock).unwrap();
        // the journaled attempt; where it succeeded, the replacement is what the kill leaves
        let _ = txn.link(&new, &exe);
        // the kill lands with the running image renamed away and nothing placed
        let _ = fs::rename(&exe, h.installer().join("previous").join(".aside-t-x"));
        assert!(!exe.exists());
        // the replay (rollback is what `recover` runs for a txn whose owner died)
        txn.rollback().unwrap();
        assert_eq!(fs::read(&exe).unwrap(), old);
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn windows_a_held_file_is_waited_for_then_given_up_on() {
        let (_t, h) = home();
        let held = h.installer().join("previous").join(".aside-t-held.exe");
        fs::write(&held, "x").unwrap();
        // an open handle without FILE_SHARE_DELETE (the default for std's File on Windows
        // grants it, so ask for none)
        use std::os::windows::fs::OpenOptionsExt;
        let f = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&held)
            .unwrap();
        let t0 = Instant::now();
        assert_eq!(sweep_aside(&h, Duration::from_millis(400)), 1);
        assert!(t0.elapsed() >= Duration::from_millis(400));
        drop(f);
        assert_eq!(sweep_aside(&h, Duration::from_secs(1)), 0);
    }
}
