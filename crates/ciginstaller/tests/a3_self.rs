// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0
//! `a3_self_update` (SPEC 14.3): the fixture ships an installer 0.1.1 (a fake that supports
//! `__selfcheck`); the swap is atomic; a 0.1.1 whose self-check fails leaves the old binary back
//! with E1853; a kill at every journal point leaves a working `bin/ciginstaller`.
#![cfg(unix)]

mod common;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use cigi_testkit::{Fixture, Snapshot};
use common::*;

/// The running (real) installer is 0.1.0; the channel offers 0.1.1.
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
    warm(&fx);
    fx
}

/// The first good command of the installed binary marks the build healthy (SPEC 12.4); do it
/// now, so a later "nothing changed" comparison is not about that.
fn warm(fx: &Fixture) {
    ok(&output(managed(fx).args(["--yes", "list"])));
    assert_eq!(state(fx)["installer"]["healthy"], true);
}

fn bin(fx: &Fixture, name: &str) -> PathBuf {
    fx.cigscript().join("bin").join(name)
}

/// A command that runs the installer the home holds (`bin/ciginstaller`), not the build under test.
fn managed(fx: &Fixture) -> Command {
    let base = fx.cigi();
    let mut c = Command::new(bin(fx, "ciginstaller"));
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

/// `Command::output`, tried again while exec fails with "text file busy" (a binary written a
/// moment ago can still be open in a child another test thread forked).
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

fn selfcheck(fx: &Fixture) -> (Option<i32>, String) {
    let o = output(managed(fx).arg("__selfcheck"));
    (o.status.code(), out(&o))
}

/// A command that takes the lock and runs `recover` (a read-only `list` does not), and changes
/// nothing when the carton is there: the next command after a kill.
fn recover(fx: &Fixture) -> Output {
    cigi(fx, &["install", "carton", "--carton", "0.1.0"])
}

/// What a kill must leave whole, plus the previous binary.
fn view(fx: &Fixture) -> Snapshot {
    let root = fx.cigscript();
    let mut s = Snapshot::default();
    for p in [
        "bin",
        "toolchains",
        "store/objects",
        "humidor",
        "installer/previous",
    ] {
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

fn state(fx: &Fixture) -> serde_json::Value {
    let t = std::fs::read_to_string(fx.cigscript().join("installer/state.json")).unwrap();
    serde_json::from_str(&t).unwrap()
}

fn id(path: &PathBuf) -> (u64, u64) {
    let m = std::fs::metadata(path).unwrap();
    (m.ino(), m.len())
}

#[test]
fn a3_self_update() {
    let fx = setup(false);
    let before = view(&fx);
    let old = id(&bin(&fx, "ciginstaller"));
    assert_eq!(selfcheck(&fx), (Some(0), "ciginstaller 0.1.0".to_string()));

    // --check changes nothing and says what is on offer
    let o = output(managed(&fx).args(["--yes", "self", "update", "--check"]));
    ok(&o);
    assert!(
        text(&o).contains("ciginstaller 0.1.1 is available (this is 0.1.0)"),
        "{}",
        text(&o)
    );
    view(&fx).assert_same(&before);

    // a build from a source tree never replaces itself, and says why
    let o = cigi(&fx, &["self", "update"]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(text(&o).contains("E1853"), "{}", text(&o));
    assert!(text(&o).contains("development build"), "{}", text(&o));
    view(&fx).assert_same(&before);

    // the update, with a watcher that looks at bin/ the whole time: the file is always there and
    // is always exactly the old binary or the new one (rename over, never a partial file)
    let stop = Arc::new(AtomicBool::new(false));
    let seen = {
        let (stop, paths) = (
            Arc::clone(&stop),
            [bin(&fx, "ciginstaller"), bin(&fx, "cigi")],
        );
        std::thread::spawn(move || {
            let mut seen = std::collections::BTreeSet::new();
            let mut missing = 0u32;
            while !stop.load(Ordering::SeqCst) {
                for p in &paths {
                    match std::fs::metadata(p) {
                        Ok(m) => {
                            seen.insert((m.ino(), m.len()));
                        }
                        Err(_) => missing += 1,
                    }
                }
            }
            (seen, missing)
        })
    };
    let o = output(managed(&fx).args(["--yes", "self", "update"]));
    stop.store(true, Ordering::SeqCst);
    let (seen, missing) = seen.join().unwrap();
    ok(&o);
    assert!(
        text(&o).contains("ciginstaller updated 0.1.0 -> 0.1.1"),
        "{}",
        text(&o)
    );
    let new = id(&bin(&fx, "ciginstaller"));
    assert_ne!(new, old);
    assert_eq!(missing, 0, "bin/ciginstaller was missing for a moment");
    assert!(
        seen.iter().all(|s| *s == old || *s == new),
        "a third file was seen in bin/: {seen:?} (old {old:?}, new {new:?})"
    );

    // the new binary is in place under both names; the old one is kept
    assert_eq!(selfcheck(&fx), (Some(0), "ciginstaller 0.1.1".to_string()));
    assert_eq!(id(&bin(&fx, "cigi")), new);
    assert_eq!(
        id(&fx.cigscript().join("installer/previous/ciginstaller")),
        old
    );
    assert!(fx
        .cigscript()
        .join("toolchains/ciginstaller/0.1.1/.cigi-receipt.json")
        .is_file());

    // every shim now is a link of the new cigi-shim object
    let shim_obj = id(&fx
        .cigscript()
        .join("toolchains/ciginstaller/0.1.1/bin/cigi-shim"));
    for n in ["cig", "cigo"] {
        assert_eq!(id(&bin(&fx, n)), shim_obj, "bin/{n}");
    }

    // state: the new version, the old kept, not healthy until a good run
    let s = state(&fx);
    assert_eq!(s["installer"]["version"], "0.1.1");
    assert_eq!(s["installer"]["previous"], "0.1.0");
    assert_eq!(s["installer"]["healthy"], false);
    assert_eq!(
        s["history"].as_array().unwrap().last().unwrap()["what"],
        "self update"
    );

    // the home is sound for the next command, and nothing needed recovering
    let o = recover(&fx);
    ok(&o);
    assert!(!text(&o).contains("E1832"), "{}", text(&o));

    // asked again from a dev build: still refused, still unchanged (idempotent)
    let after = view(&fx);
    let o = cigi(&fx, &["self", "update"]);
    assert_eq!(o.status.code(), Some(2));
    view(&fx).assert_same(&after);
}

#[test]
fn a3_self_update_failed_selfcheck_puts_the_old_binary_back() {
    let fx = setup(true);
    let before = view(&fx);
    let old = id(&bin(&fx, "ciginstaller"));
    let o = output(managed(&fx).args(["--yes", "self", "update"]));
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
    let t = text(&o);
    assert!(t.contains("E1853"), "{t}");
    assert!(t.contains("failed its self-check"), "{t}");
    assert!(t.contains("0.1.0 was put back"), "{t}");
    // exactly as before: binaries, shims, toolchains, store, state, index; no previous/
    view(&fx).assert_same(&before);
    assert_eq!(id(&bin(&fx, "ciginstaller")).1, old.1);
    assert_eq!(selfcheck(&fx), (Some(0), "ciginstaller 0.1.0".to_string()));
    assert!(!fx.cigscript().join("installer/previous").exists());
    // the undo was complete: the next command has nothing to recover
    let o = recover(&fx);
    ok(&o);
    assert!(!text(&o).contains("E1832"), "{}", text(&o));
}

#[test]
fn a3_self_update_when_nothing_newer_is_on_offer() {
    let mut fx = fixture();
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("ciginstaller", "0.1.0")]);
    fx.publish("stable");
    ok(&cigi(&fx, &["install", "carton"]));
    warm(&fx);
    let before = view(&fx);
    let o = output(managed(&fx).args(["--yes", "self", "update"]));
    ok(&o);
    assert!(
        text(&o).contains("ciginstaller 0.1.0 is the newest"),
        "{}",
        text(&o)
    );
    view(&fx).assert_same(&before);
}

/// Kill the update at every journal point: after the next command `bin/ciginstaller` works and
/// is the old build (undone) or the new one (finished), and state, shims and toolchains agree
/// with it.
#[test]
fn a3_self_update_killed_at_every_point() {
    let mut crashes = 0;
    let (mut old_seen, mut new_seen) = (0, 0);
    for phase in [
        "mkdir", "store", "link", "move", "write", "delete", "flip", "flipped", "commit",
    ] {
        for n in 1..80 {
            let fx = setup(false);
            let before = view(&fx);
            let o = output(
                managed(&fx)
                    .env("CIGI_FAULT", format!("{phase}:{n}"))
                    .args(["--yes", "self", "update"]),
            );
            if o.status.code().is_some() {
                break; // this phase has fewer than n lines
            }
            crashes += 1;
            let tag = format!("{phase}:{n}");
            // mid-crash: whatever is in bin/ starts
            let (code, said) = selfcheck(&fx);
            assert_eq!(code, Some(0), "{tag}: {said}");
            assert!(
                said == "ciginstaller 0.1.0" || said == "ciginstaller 0.1.1",
                "{tag}: {said}"
            );
            // the next command recovers
            let o = recover(&fx);
            ok(&o);
            let (code, said) = selfcheck(&fx);
            assert_eq!(code, Some(0), "{tag}: {said}");
            let s = state(&fx);
            let version = s["installer"]["version"].as_str().unwrap().to_string();
            assert_eq!(said, format!("ciginstaller {version}"), "{tag}");
            assert_eq!(
                id(&bin(&fx, "cigi")),
                id(&bin(&fx, "ciginstaller")),
                "{tag}"
            );
            if version == "0.1.0" {
                old_seen += 1;
                let d = view(&fx).diff(&before);
                assert!(d.is_empty(), "{tag}: {d:?}");
            } else {
                new_seen += 1;
                assert_eq!(s["installer"]["previous"], "0.1.0", "{tag}");
                assert!(
                    fx.cigscript()
                        .join("installer/previous/ciginstaller")
                        .is_file(),
                    "{tag}"
                );
                let shim_obj = id(&fx
                    .cigscript()
                    .join("toolchains/ciginstaller/0.1.1/bin/cigi-shim"));
                for n in ["cig", "cigo"] {
                    assert_eq!(id(&bin(&fx, n)), shim_obj, "{tag}: bin/{n}");
                }
            }
        }
    }
    assert!(crashes >= 10, "only {crashes} fault points reached");
    assert!(
        old_seen > 0 && new_seen > 0,
        "old {old_seen}, new {new_seen}"
    );
}

/// SPEC 12.2 step 1: `update` updates the installer first when the channel offers a newer one,
/// then carries on as the new binary with the same arguments (the fake 0.1.1 says what it got).
#[test]
fn a3_update_updates_the_installer_first_and_carries_on_as_it() {
    let fx = setup(false);
    let o = output(managed(&fx).args(["--yes", "--plain", "update"]));
    let t = text(&o);
    assert!(t.contains("ciginstaller updated 0.1.0 -> 0.1.1"), "{t}");
    assert!(t.contains("ciginstaller 0.1.1 args="), "{t}");
    assert!(t.contains("update"), "{t}");
    assert_eq!(o.status.code(), Some(0), "{t}");
    assert_eq!(state(&fx)["installer"]["version"], "0.1.1");
}
