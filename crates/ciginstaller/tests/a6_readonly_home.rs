// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! A family home that cannot be written where a command runs (inside a cig box or a cigo task, the
//! home is read-only): every command that changes it stops with `E1905` and writes nothing, and
//! the commands that only read answer for that home.

// Unix only: the home is made read-only through Unix modes.
#![cfg(unix)]

mod common;

use std::os::unix::fs::PermissionsExt;
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

/// Every folder under `root` (and `root` itself) to `mode`.
fn chmod_dirs(root: &Path, mode: u32) {
    for e in walk(root) {
        if e.is_dir() {
            std::fs::set_permissions(&e, std::fs::Permissions::from_mode(mode)).unwrap();
        }
    }
}

fn walk(root: &Path) -> Vec<std::path::PathBuf> {
    let mut out = vec![root.to_path_buf()];
    let mut i = 0;
    while i < out.len() {
        let p = out[i].clone();
        if p.is_dir() && !p.is_symlink() {
            if let Ok(rd) = std::fs::read_dir(&p) {
                for e in rd.flatten() {
                    out.push(e.path());
                }
            }
        }
        i += 1;
    }
    out
}

/// The home read-only for the length of `f`, put back after (also when `f` panics).
fn read_only<T>(fx: &Fixture, f: impl FnOnce() -> T) -> Option<T> {
    struct Back<'a>(&'a Path);
    impl Drop for Back<'_> {
        fn drop(&mut self) {
            chmod_dirs(self.0, 0o755);
        }
    }
    let root = fx.cigscript();
    chmod_dirs(&root, 0o555);
    let _back = Back(&root);
    // a user the modes do not stop (root) cannot see this refusal
    let probe = root.join(".probe");
    if std::fs::write(&probe, b"x").is_ok() {
        let _ = std::fs::remove_file(&probe);
        eprintln!("skipped: the modes do not stop this user");
        return None;
    }
    Some(f())
}

#[test]
fn a6_every_change_to_a_read_only_home_is_e1905_and_writes_nothing() {
    let fx = setup();
    let before = Snapshot::take(&fx.cigscript());
    let ran = read_only(&fx, || {
        for args in [
            vec!["install", "carton"],
            vec!["update"],
            vec!["repair"],
            vec!["config", "set", "allow_single_source", "false"],
            vec!["--yes-irreversible", "uninstall", "--all"],
            vec!["uninstall", "cigo"],
            vec!["gc"],
            vec!["shims", "sync"],
        ] {
            let o = cigi(&fx, &args);
            let t = text(&o);
            assert_eq!(o.status.code(), Some(2), "{args:?}: {t}");
            assert!(t.contains("E1905"), "{args:?}: {t}");
            assert!(
                t.contains("run the same command outside the task"),
                "{args:?}: {t}"
            );
        }
    });
    if ran.is_some() {
        Snapshot::take(&fx.cigscript()).assert_same(&before);
    }
}

#[test]
fn a6_a_read_only_home_still_answers_what_only_reads() {
    let fx = setup();
    read_only(&fx, || {
        for args in [
            vec!["status"],
            vec!["config", "get", "allow_single_source"],
            vec!["which", "cig"],
            vec!["list"],
        ] {
            let o = cigi(&fx, &args);
            let t = text(&o);
            assert_eq!(o.status.code(), Some(0), "{args:?}: {t}");
            assert!(!t.contains("E1905"), "{args:?}: {t}");
        }
        let o = cigi(&fx, &["which", "cig"]);
        assert!(text(&o).contains("1.20.0"), "{}", text(&o));
    });
}
