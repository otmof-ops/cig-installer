// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

use super::pins::pin_from_toml;

#[test]
fn toml_pin_ignores_arrays_and_strings_that_look_like_tables() {
    let t = "m = [\n  [1, 2],\n  [3, 4],\n]\ncarton = \"0.3\" # pin\n[deps]\n";
    assert_eq!(pin_from_toml(t), Some("0.3".into()));
    let s = "doc = \"\"\"\n[not a table]\ncarton = \"9\"\n\"\"\"\ncarton = '0.4'\n";
    assert_eq!(pin_from_toml(s), Some("0.4".into()));
    let l = "doc = '''\n[x]\n'''\ncarton = \"1\"\n";
    assert_eq!(pin_from_toml(l), Some("1".into()));
    // brackets in comments and strings do not move the depth
    let c = "a = \"[\" # [\nb = 1 # ]\ncarton = \"2\"\n";
    assert_eq!(pin_from_toml(c), Some("2".into()));
    // an inline table spread over lines
    let i = "t = {\n x = [1],\n}\ncarton = \"5\"\n";
    assert_eq!(pin_from_toml(i), Some("5".into()));
    // a real table ends the top level
    assert_eq!(pin_from_toml("a = 1\n[t]\ncarton = \"6\"\n"), None);
    // a key inside an open array is not the key
    assert_eq!(pin_from_toml("m = [\ncarton = \"7\",\n]\n"), None);
}

/// The gap between the checks and the open (the security re-review of PR #37): even a fifo reached by
/// the open itself returns at once, and the check on the opened file then refuses it.
#[cfg(target_os = "linux")]
#[test]
fn opening_a_fifo_never_waits_for_a_writer() {
    let dir = std::env::temp_dir().join(format!("cigi-shim-fifo-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let fifo = dir.join(".cig-carton");
    let made = std::process::Command::new("mkfifo").arg(&fifo).status();
    if !made.is_ok_and(|s| s.success()) {
        return; // no mkfifo here: nothing to prove
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let p = fifo.clone();
    std::thread::spawn(move || {
        let r = super::open_nonblocking(&p).map(|f| f.metadata().map(|m| m.is_file()));
        let _ = tx.send(r.is_ok());
    });
    let opened = rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("the open of a fifo waited for a writer");
    assert!(opened);
    let _ = std::fs::remove_dir_all(&dir);
}
