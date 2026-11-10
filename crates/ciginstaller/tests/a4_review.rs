// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! Review fixes for phase 4: names from state, repair of a toolchain with no receipt, what
//! `uninstall --all` says it keeps, read-only dry runs, `gc` for E1865, the ownership check,
//! symlinks, and a doctor that writes nothing.

// Unix only: review-fix tests build symlinks, set Unix modes and drive shell-script tools.
// The Windows runtime drafts add their own equivalents.
#![cfg(unix)]

mod common;

use std::os::unix::fs::symlink;
use std::path::Path;

use cigi_testkit::{Fixture, Snapshot};
use common::{cigi, fixture, ok, text};
use serde_json::Value;

fn setup() -> Fixture {
    let mut fx = fixture();
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.16.0")]);
    fx.carton("0.2.0", &[("cig", "1.21.0"), ("cigo", "1.17.0")]);
    fx.carton("0.3.0", &[("cig", "1.22.0"), ("cigo", "1.18.0")]);
    fx.publish("stable");
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    fx
}

fn json(o: &std::process::Output) -> Value {
    let line = String::from_utf8_lossy(&o.stdout)
        .lines()
        .last()
        .unwrap_or("")
        .to_string();
    serde_json::from_str::<Value>(&line).unwrap_or_else(|e| panic!("{e}: {}", text(o)))
}

fn doctor(fx: &Fixture, extra: &[&str]) -> (i32, Value) {
    let mut args = vec!["--json", "doctor"];
    args.extend_from_slice(extra);
    let o = cigi(fx, &args);
    (o.status.code().unwrap_or(-1), json(&o)["data"].clone())
}

fn actions<'a>(d: &'a Value, section: &str) -> Vec<&'a Value> {
    d["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["section"] == section && f["severity"] == "action")
        .collect()
}

fn run_fix(fx: &Fixture, fix: &str) {
    let rest = fix.strip_prefix("ciginstaller ").expect(fix);
    let args: Vec<&str> = rest.split_whitespace().collect();
    ok(&cigi(fx, &args));
}

fn tc(fx: &Fixture, rest: &str) -> std::path::PathBuf {
    fx.cigscript().join("toolchains").join(rest)
}

fn receipt(fx: &Fixture) -> std::path::PathBuf {
    tc(fx, "cigo/1.16.0/.cigi-receipt.json")
}

/// A toolchain tree without its receipt (which carries a time).
fn tree_without_receipt(fx: &Fixture, tool_ver: &str) -> Snapshot {
    Snapshot::take(&tc(fx, tool_ver)).without(&[".cigi-receipt.json"])
}

fn crash_an_install(fx: &Fixture) {
    let o = fx
        .cigi()
        .env("CIGI_ALLOW_SINGLE_SOURCE", "true")
        .env("CIGI_FAULT", "link:3")
        .args(["--yes", "install", "carton", "--carton", "0.2.0"])
        .output()
        .unwrap();
    assert!(
        o.status.code().is_none(),
        "the install should have been killed"
    );
}

// ----------------------------------------------------- 2: shim names come from state

#[test]
fn a4_shim_names_come_from_state_not_from_receipts() {
    let fx = setup();
    let before = tree_without_receipt(&fx, "cigo/1.16.0");
    for damage in ["delete", "garbage"] {
        match damage {
            "delete" => std::fs::remove_file(receipt(&fx)).unwrap(),
            _ => std::fs::write(receipt(&fx), "{ not json").unwrap(),
        }
        // `shims sync` must not take the shim of an installed tool away
        let o = cigi(&fx, &["--json", "shims", "sync"]);
        ok(&o);
        assert!(
            fx.cigscript().join("bin/cigo").exists(),
            "{damage}: the shim went"
        );
        assert!(
            json(&o)["data"]["shims"]
                .as_array()
                .unwrap()
                .iter()
                .any(|n| n == "cigo"),
            "{damage}"
        );
        assert!(
            json(&o)["data"]["removed"].as_array().unwrap().is_empty(),
            "{damage}"
        );
        // the doctor calls the receipt a finding, with a fix that clears it (fix 3)
        let (code, d) = doctor(&fx, &[]);
        assert_eq!(code, 2, "{damage}: {d:#}");
        let f = actions(&d, "toolchains");
        assert_eq!(f.len(), 1, "{damage}: {d:#}");
        assert_eq!(f[0]["code"], "E1831");
        let fix = f[0]["fix"].as_str().unwrap().to_string();
        assert_eq!(fix, "ciginstaller repair --tool cigo");
        run_fix(&fx, &fix);
        let (code, d) = doctor(&fx, &[]);
        assert_eq!(code, 0, "{damage}: {d:#}");
        // the toolchain is what it was, with a receipt again
        assert!(receipt(&fx).is_file());
        tree_without_receipt(&fx, "cigo/1.16.0").assert_same(&before);
        let o = fx.run_shim("cigo", &fx.project, &[]);
        ok(&o);
    }
}

// ---------------------------------------- 3: repair rebuilds a toolchain with no receipt

#[test]
fn a4_repair_rebuilds_a_toolchain_whose_receipt_names_a_path_outside_it() {
    let fx = setup();
    let before = tree_without_receipt(&fx, "cigo/1.16.0");
    let p = receipt(&fx);
    let mut v: Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
    v["files"][0]["path"] = "../../../../victim".into();
    std::fs::set_permissions(&p, std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap();
    std::fs::write(&p, v.to_string()).unwrap();
    let (code, d) = doctor(&fx, &[]);
    assert_eq!(code, 2, "{d:#}");
    run_fix(&fx, actions(&d, "toolchains")[0]["fix"].as_str().unwrap());
    let (code, d) = doctor(&fx, &[]);
    assert_eq!(code, 0, "{d:#}");
    tree_without_receipt(&fx, "cigo/1.16.0").assert_same(&before);
    assert!(!fx.user_home.join("victim").exists());
}

#[test]
fn a4_repair_without_a_manifest_is_e1860_with_a_fix_that_names_the_tool() {
    let fx = setup();
    std::fs::remove_file(receipt(&fx)).unwrap();
    // no manifest and no artifact: the cache and the mirror are gone
    std::fs::remove_dir_all(fx.cigscript().join("installer/cache/manifests")).unwrap();
    std::fs::remove_dir_all(&fx.releases).unwrap();
    let before = Snapshot::take(&fx.user_home).without(&[
        ".cigscript/installer/lock",
        ".cigscript/installer.lock",
        ".cigscript/installer/lock.pid",
    ]);
    let o = cigi(&fx, &["--json", "repair", "--tool", "cigo"]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(text(&o).contains("E1860"), "{}", text(&o));
    assert!(
        text(&o).contains("ciginstaller repair --tool cigo"),
        "{}",
        text(&o)
    );
    Snapshot::take(&fx.user_home)
        .without(&[
            ".cigscript/installer/lock",
            ".cigscript/installer.lock",
            ".cigscript/installer/lock.pid",
            ".cigscript/installer/cache",
        ])
        .assert_same(&before.without(&[".cigscript/installer/cache"]));
    // the finding stays, and so does cig, which has nothing wrong
    assert!(tc(&fx, "cig/1.20.0/bin/cig").exists());
}

// --------------------------------------------- 4: what stays, what goes with installer/

#[test]
fn a4_uninstall_all_says_what_stays_and_what_goes_with_the_installer() {
    let fx = setup();
    let root = fx.cigscript();
    std::fs::create_dir_all(root.join("runs/r1")).unwrap();
    std::fs::create_dir_all(root.join("tin")).unwrap();
    std::fs::write(root.join("config"), "mode = tidy\n").unwrap();
    ok(&cigi(&fx, &["config", "set", "keep_days", "30"]));
    assert!(root.join("installer/config").is_file());
    let o = cigi(&fx, &["--json", "uninstall", "--all", "--dry-run"]);
    ok(&o);
    let d = json(&o)["data"].clone();
    let stays: Vec<&str> = d["stays"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(
        stays.contains(&"runs/") && stays.contains(&"tin/") && stays.contains(&"config"),
        "{stays:?}"
    );
    assert!(!stays.iter().any(|s| s.contains("installer")), "{stays:?}");
    let goes = d["would_remove"].as_array().unwrap();
    let inst = goes
        .iter()
        .map(|v| v.as_str().unwrap())
        .find(|p| p.contains("/installer"))
        .expect("installer/ is listed");
    for word in ["installer/config", "cache", "journals"] {
        assert!(inst.contains(word), "{inst}");
    }
    // the text says it too, and names the home
    let t = text(&cigi(&fx, &["--plain", "uninstall", "--all", "--dry-run"]));
    assert!(t.contains("stays in"), "{t}");
    assert!(t.contains(&root.display().to_string()), "{t}");
    // and what it says is what happens
    ok(&cigi(&fx, &["uninstall", "--all", "--yes-irreversible"]));
    assert!(!root.join("installer").exists());
    assert!(root.join("runs/r1").is_dir() && root.join("tin").is_dir());
    assert_eq!(
        std::fs::read_to_string(root.join("config")).unwrap(),
        "mode = tidy\n"
    );
}

// -------------------------------------------------------------- 5: dry runs are read-only

#[test]
fn a4_dry_runs_take_no_lock_make_no_layout_and_settle_nothing() {
    let fx = setup();
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.3.0"]));
    crash_an_install(&fx);
    // a directory the layout would make again
    let dl = fx.cigscript().join("installer/cache/downloads");
    let _ = std::fs::remove_dir_all(&dl);
    let before = Snapshot::take(&fx.user_home); // the lock's pid is part of it
    for args in [
        &["--json", "repair", "--dry-run"][..],
        &["--json", "uninstall", "cigo", "--dry-run"][..],
        &["--json", "remove", "0.3.0", "--dry-run"][..],
    ] {
        let o = cigi(&fx, args);
        ok(&o);
        let d = json(&o)["data"].clone();
        assert_eq!(d["dry_run"], true, "{args:?}: {d:#}");
        // it says what is waiting instead of settling it
        let waiting = d["interrupted"]
            .as_array()
            .unwrap_or_else(|| panic!("{args:?}: {d:#}"));
        assert_eq!(waiting.len(), 1, "{args:?}: {d:#}");
        assert!(
            !text(&o).contains("E1832"),
            "{args:?}: it recovered: {}",
            text(&o)
        );
        Snapshot::take(&fx.user_home).assert_same(&before);
        assert!(!dl.exists(), "{args:?}: ensure_layout ran");
    }
}

// ------------------------------------------------------------------ 6: E1865 and gc

#[test]
fn a4_gc_discards_the_journal_of_a_cut_short_uninstall_and_only_that() {
    let fx = setup();
    let o = fx
        .cigi()
        .env("CIGI_ALLOW_SINGLE_SOURCE", "true")
        .env("CIGI_FAULT", "uninstall-aside:1")
        .args(["--yes-irreversible", "uninstall", "--all"])
        .output()
        .unwrap();
    assert!(o.status.code().is_none());
    let sib = |n: &str| fx.user_home.join(n);
    let aside: Vec<String> = std::fs::read_dir(&fx.user_home)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(".cigscript-uninstall-"))
        .collect();
    assert_eq!(aside.len(), 1);
    // a look-alike that is not exactly that name is never touched
    std::fs::create_dir_all(sib(".cigscript-uninstall-bogus")).unwrap();
    std::fs::create_dir_all(sib(".cigscript-uninstall-20260101T000000-abcdef.bak")).unwrap();
    let (_, d) = doctor(&fx, &[]);
    let f = actions(&d, "uninstall");
    assert_eq!(f.len(), 1, "{d:#}");
    assert_eq!(f[0]["code"], "E1865");
    assert_eq!(f[0]["fix"], "ciginstaller gc");
    // the installer's directory has been made again by something else: gc still clears it
    std::fs::create_dir_all(fx.cigscript().join("installer")).unwrap();
    std::fs::set_permissions(
        fx.cigscript().join("installer"),
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .unwrap();
    ok(&cigi(&fx, &["gc"]));
    assert!(!sib(&aside[0]).exists());
    assert!(sib(".cigscript-uninstall-bogus").exists());
    assert!(sib(".cigscript-uninstall-20260101T000000-abcdef.bak").exists());
}

#[test]
fn a4_gc_keeps_a_journal_that_was_kept_on_purpose() {
    let fx = setup();
    ok(&cigi(
        &fx,
        &["uninstall", "--all", "--keep-journal", "--yes-irreversible"],
    ));
    let n = || {
        std::fs::read_dir(&fx.user_home)
            .unwrap()
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(".cigscript-uninstall-")
            })
            .count()
    };
    assert_eq!(n(), 1);
    ok(&cigi(&fx, &["gc"]));
    assert_eq!(n(), 1);
}

// -------------------------------------------------------------------- 8: ownership

#[test]
fn a4_uninstall_all_refuses_a_directory_that_is_not_an_installer_home() {
    let fx = fixture();
    let root = fx.cigscript();
    // a project directory that happens to have installer/ and store/ in it
    std::fs::create_dir_all(root.join("installer")).unwrap();
    std::fs::create_dir_all(root.join("store")).unwrap();
    std::fs::write(root.join("store/important"), "data").unwrap();
    std::fs::create_dir_all(root.join("toolchains/x")).unwrap();
    let before = Snapshot::take(&fx.user_home);
    let o = cigi(&fx, &["uninstall", "--all", "--yes-irreversible"]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(text(&o).contains("E1868"), "{}", text(&o));
    Snapshot::take(&fx.user_home).assert_same(&before); // not even a lock file or a marker
                                                        // a state.json that is not ours
                                                        // a state.json that is not ours: refused, as E1830 where the state is read at start, or E1868
    for junk in ["{\"hello\": 1}", "[1, 2]", "not json"] {
        std::fs::write(root.join("installer/state.json"), junk).unwrap();
        for args in [
            &["uninstall", "--all", "--yes-irreversible"][..],
            &["uninstall", "--all", "--dry-run"][..],
        ] {
            let o = cigi(&fx, args);
            assert!(!o.status.success(), "{junk}: {}", text(&o));
            assert!(
                text(&o).contains("E1868") || text(&o).contains("E1830"),
                "{junk}: {}",
                text(&o)
            );
        }
    }
    assert!(root.join("store/important").exists());
}

#[test]
fn a4_uninstall_all_checks_the_marker_and_accepts_a_home_made_before_it_existed() {
    let fx = setup();
    let marker = fx.cigscript().join("installer/.owner");
    assert!(std::fs::read_to_string(&marker)
        .unwrap()
        .starts_with("ciginstaller"));
    // a marker that says something else
    std::fs::write(&marker, "someone else's\n").unwrap();
    let o = cigi(&fx, &["uninstall", "--all", "--yes-irreversible"]);
    assert!(text(&o).contains("E1868"), "{}", text(&o));
    assert!(fx.cigscript().join("toolchains").exists());
    // no marker at all: a home from before the marker, accepted on its state
    std::fs::remove_file(&marker).unwrap();
    ok(&cigi(&fx, &["uninstall", "--all", "--yes-irreversible"]));
    assert!(!fx.cigscript().join("installer").exists());
}

// ------------------------------------------------------------------------ 12: symlinks

#[test]
fn a4_symlinks_in_bin_and_in_toolchains_are_removed_without_being_followed() {
    let fx = setup();
    let precious = fx.tmp.path().join("precious");
    std::fs::write(&precious, "keep me").unwrap();
    // an extra link in a toolchain: repair removes the link
    let lnk = tc(&fx, "cig/1.20.0/bin/lnk");
    symlink(&precious, &lnk).unwrap();
    let (code, d) = doctor(&fx, &[]);
    assert_eq!(code, 2, "{d:#}");
    ok(&cigi(&fx, &["repair"]));
    assert!(std::fs::symlink_metadata(&lnk).is_err());
    assert_eq!(std::fs::read_to_string(&precious).unwrap(), "keep me");
    // a link where a shim should be: uninstall --all unlinks it
    let shim = fx.cigscript().join("bin/cigo");
    std::fs::remove_file(&shim).unwrap();
    symlink(&precious, &shim).unwrap();
    ok(&cigi(&fx, &["uninstall", "--all", "--yes-irreversible"]));
    assert!(!fx.cigscript().join("bin").exists());
    assert_eq!(std::fs::read_to_string(&precious).unwrap(), "keep me");
}

#[test]
fn a4_uninstall_all_with_a_profile_that_is_a_link_keeps_the_link() {
    let mut fx = fixture();
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.16.0")]);
    fx.publish("stable");
    // a dotfiles repository outside the home, the profile a link into it
    let dot = fx.tmp.path().join("dotfiles");
    std::fs::create_dir_all(&dot).unwrap();
    let real = dot.join("profile");
    std::fs::write(&real, "export FOO=1\n").unwrap();
    symlink(&real, fx.user_home.join(".profile")).unwrap();
    ok(&cigi(&fx, &["install", "carton"]));
    assert!(std::fs::read_to_string(&real)
        .unwrap()
        .contains("# >>> cigscript >>>"));
    ok(&cigi(&fx, &["uninstall", "--all", "--yes-irreversible"]));
    assert!(std::fs::symlink_metadata(fx.user_home.join(".profile"))
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(std::fs::read_to_string(&real).unwrap(), "export FOO=1\n");
}

// ------------------------------------------------------------- 14: doctor writes nothing

#[test]
fn a4_doctor_network_writes_nothing_and_a_loose_home_is_reported() {
    // no home at all: --network makes none
    let fx = fixture();
    let mut fx2 = setup();
    let _ = &mut fx2;
    let (code, _) = doctor(&fx, &["--network"]);
    assert_eq!(code, 0);
    assert!(!fx.cigscript().exists(), "doctor --network made the home");
    // a home: the cache is not rewritten, the manifest still checked
    let fx = setup();
    let meta = fx
        .cigscript()
        .join("installer/cache/manifests/stable/fetched.json");
    let mut v: Value = serde_json::from_slice(&std::fs::read(&meta).unwrap()).unwrap();
    v["fetched_at"] = "2020-01-01T00:00:00Z".into();
    std::fs::write(&meta, v.to_string()).unwrap();
    let before = Snapshot::take(&fx.user_home);
    let (_, d) = doctor(&fx, &["--network"]);
    assert!(d["findings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["section"] == "matrix"));
    Snapshot::take(&fx.user_home).assert_same(&before);
    // modes: the home and installer/ are 0700; a looser one is reported and then tightened
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions();
    for d in [fx.cigscript(), fx.cigscript().join("installer")] {
        std::fs::set_permissions(&d, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    }
    let (code, d) = doctor(&fx, &[]);
    assert_eq!(code, 2, "{d:#}");
    let f = actions(&d, "home");
    assert_eq!(f.len(), 2, "{d:#}");
    assert_eq!(f[0]["fix"], "ciginstaller repair");
    run_fix(&fx, "ciginstaller repair");
    for d in [fx.cigscript(), fx.cigscript().join("installer")] {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(mode(&d).mode() & 0o777, 0o700, "{}", d.display());
    }
    let (code, d) = doctor(&fx, &[]);
    assert_eq!(code, 0, "{d:#}");
}

// ---------------------------------------------------------------------- 15: --tools

#[test]
fn a4_doctor_tools_skips_a_tool_whose_toolchain_is_damaged() {
    let fx = setup();
    let file = tc(&fx, "cig/1.20.0/bin/cig");
    let tmp = file.with_extension("new");
    std::fs::write(&tmp, "#!/bin/sh\necho tampered args=$*\n").unwrap();
    std::fs::set_permissions(&tmp, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    std::fs::rename(&tmp, &file).unwrap();
    let (_, d) = doctor(&fx, &["--tools"]);
    let tools = d["tools"].as_array().unwrap();
    let cig = tools.iter().find(|t| t["tool"] == "cig 1.20.0").unwrap();
    assert_eq!(cig["ok"], false);
    assert_eq!(cig["code"], "E1864");
    assert!(cig["note"].as_str().unwrap().contains("not run"), "{cig}");
    assert!(
        !cig["note"].as_str().unwrap().contains("tampered"),
        "the changed binary was run: {d:#}"
    );
    let cigo = tools.iter().find(|t| t["tool"] == "cigo 1.16.0").unwrap();
    assert_eq!(cigo["ok"], true, "{cigo}");
}
