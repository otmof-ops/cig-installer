// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! What the library refuses to follow or trust: receipt paths, links where files or directories
//! should be, humidor names, profiles that are links, the owner marker and the home's modes.

// Unix only: every test here builds symlinks or reads and sets Unix modes. The Windows
// equivalents (junctions, ACL owner) are the Windows runtime drafts' tests.
#![cfg(unix)]

mod common;

use std::fs;
use std::os::unix::fs::symlink;

use cigi_home::path::block_state;
use cigi_home::*;
use common::*;

fn install_cigo(fx: &Fx) -> Receipt {
    let mut t = fx.txn("install cigo");
    let files = fx.staged(&[
        ("bin/cigo", b"#!/bin/sh\necho cigo\n", true),
        ("orchestrate.cig", b"shared", false),
    ]);
    let r = materialize(&mut t, &fx.home, "cigo", &v("1.17.0"), &files, prov()).unwrap();
    t.commit().unwrap();
    r
}

fn rewrite_receipt(fx: &Fx, edit: impl Fn(&mut serde_json::Value)) {
    let p = fx
        .home
        .toolchain("cigo", &v("1.17.0"))
        .join(".cigi-receipt.json");
    let mut val: serde_json::Value = serde_json::from_slice(&fs::read(&p).unwrap()).unwrap();
    edit(&mut val);
    fs::set_permissions(&p, std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap();
    fs::write(&p, val.to_string()).unwrap();
}

// ----------------------------------------------------------------- F3: receipt paths

#[test]
fn a_receipt_path_that_leaves_the_toolchain_makes_the_receipt_unreadable() {
    for bad in [
        "../../../x",
        "/etc/passwd",
        "bin/../../x",
        "",
        ".cigi-receipt.json",
        "a//b",
        "./a",
        "a\\b",
    ] {
        let fx = fx();
        install_cigo(&fx);
        assert!(read_receipt(&fx.home, "cigo", &v("1.17.0")).is_ok());
        rewrite_receipt(&fx, |r| r["files"][1]["path"] = bad.into());
        let e = read_receipt(&fx.home, "cigo", &v("1.17.0")).unwrap_err();
        assert!(e.to_string().contains("not a plain path"), "{bad}: {e}");
        // an unreadable receipt is NoReceipt to the verifier, and the toolchain is not installed
        assert_eq!(
            verify_toolchain(&fx.home, "cigo", &v("1.17.0")),
            vec![Damage::NoReceipt],
            "{bad}"
        );
        assert!(installed(&fx.home).is_empty(), "{bad}");
    }
    // a shim path too
    let fx = fx();
    install_cigo(&fx);
    rewrite_receipt(&fx, |r| r["shims"][0]["path"] = "../../bin/evil".into());
    assert!(read_receipt(&fx.home, "cigo", &v("1.17.0")).is_err());
}

#[test]
fn restore_file_refuses_a_path_outside_the_toolchain() {
    let fx = fx();
    let r = install_cigo(&fx);
    let mut f = r.files[0].clone();
    for bad in ["../../../../victim", "/tmp/victim", "bin/../../x"] {
        f.path = bad.to_string();
        let mut t = fx.txn("restore");
        let e = toolchain::restore_file(&mut t, &fx.home, "cigo", &v("1.17.0"), &f).unwrap_err();
        assert!(matches!(e, ToolchainError::BadPath(_)), "{bad}: {e}");
        t.rollback().unwrap();
    }
    assert!(!fx.tmp.path().join("victim").exists());
    assert!(!fx.tmp.path().join("u").join("victim").exists());
}

// ------------------------------------------------------------------- F5: Store::verify

#[test]
fn a_symlinked_store_object_does_not_verify() {
    let fx = fx();
    let r = install_cigo(&fx);
    let f = &r.files[1]; // orchestrate.cig
    let id = ObjectId::new(f.sha256.clone(), f.exec);
    let store = fx.home.object_store();
    assert!(store.verify(&id).is_ok());
    // the same bytes, behind a link
    let real = fx.tmp.path().join("elsewhere");
    fs::write(&real, b"shared").unwrap();
    let p = store.object_path(&id);
    fs::remove_file(&p).unwrap();
    symlink(&real, &p).unwrap();
    let e = store.verify(&id).unwrap_err();
    assert!(matches!(e, StoreError::Mismatch { .. }), "{e}");
    assert!(!store.has(&id));
}

// -------------------------------------------------------- F6: links in delete_within

#[test]
fn a_symlink_is_unlinked_not_followed_and_comes_back_on_rollback() {
    let fx = fx();
    let target = fx.tmp.path().join("precious");
    fs::write(&target, b"keep me").unwrap();
    let link = fx.home.bin().join("cigo");
    fs::create_dir_all(fx.home.bin()).unwrap();
    symlink(&target, &link).unwrap();
    let mut t = fx.txn("delete a link");
    t.delete(&link).unwrap();
    assert!(fs::symlink_metadata(&link).is_err(), "the link is gone");
    assert_eq!(fs::read(&target).unwrap(), b"keep me");
    t.rollback().unwrap();
    assert_eq!(fs::read_link(&link).unwrap(), target);
    assert_eq!(fs::read(&target).unwrap(), b"keep me");
}

#[test]
fn a_path_through_a_symlinked_directory_is_refused_with_e1869() {
    let fx = fx();
    let outside = fx.tmp.path().join("outside");
    fs::create_dir_all(outside.join("1.0.0")).unwrap();
    fs::write(outside.join("1.0.0/file"), b"x").unwrap();
    symlink(&outside, fx.home.toolchains().join("cig")).unwrap();
    let mut t = fx.txn("delete through a link");
    let e = t
        .delete(&fx.home.toolchains().join("cig").join("1.0.0"))
        .unwrap_err();
    assert_eq!(e.code(), "E1869", "{e}");
    assert!(e.to_string().contains("cig"), "{e}");
    assert!(outside.join("1.0.0/file").exists());
    // and a path that is not inside the home says so with the same code
    let e = t.delete(&fx.tmp.path().join("outside")).unwrap_err();
    assert_eq!(e.code(), "E1869", "{e}");
    t.rollback().unwrap();
}

#[test]
fn writing_through_a_symlink_is_refused_with_e1869() {
    let fx = fx();
    let target = fx.tmp.path().join("t");
    fs::write(&target, b"x").unwrap();
    let p = fx.home.installer().join("config");
    symlink(&target, &p).unwrap();
    let mut t = fx.txn("write");
    let e = t.write(&p, b"y", None).unwrap_err();
    assert_eq!(e.code(), "E1869", "{e}");
    assert_eq!(fs::read(&target).unwrap(), b"x");
    t.rollback().unwrap();
}

// ----------------------------------------------------------------------- F7: installed

#[test]
fn a_linked_toolchain_directory_is_not_an_install_and_is_not_removed_through() {
    let fx = fx();
    install_cigo(&fx);
    let outside = fx.tmp.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    // a copy of the toolchain elsewhere, reached through links at both levels
    fs::create_dir_all(outside.join("1.9.9")).unwrap();
    fs::write(outside.join("1.9.9/keep"), b"x").unwrap();
    fs::copy(
        fx.home
            .toolchain("cigo", &v("1.17.0"))
            .join(".cigi-receipt.json"),
        outside.join("1.9.9/.cigi-receipt.json"),
    )
    .unwrap();
    symlink(&outside, fx.home.toolchains().join("linked")).unwrap();
    symlink(
        fx.home.toolchain("cigo", &v("1.17.0")),
        fx.home.toolchains().join("cigo").join("1.18.0"),
    )
    .unwrap();
    assert_eq!(installed(&fx.home), vec![("cigo".to_string(), v("1.17.0"))]);
    let e = toolchain::remove_toolchain_now(&fx.home, "linked", &v("1.9.9")).unwrap_err();
    assert!(matches!(e, ToolchainError::BadPath(_)), "{e}");
    assert!(outside.join("1.9.9/keep").exists());
    let e = toolchain::remove_toolchain_now(&fx.home, "cigo", &v("1.18.0")).unwrap_err();
    assert!(matches!(e, ToolchainError::BadPath(_)), "{e}");
    assert!(fx.home.toolchain("cigo", &v("1.17.0")).exists());
}

// --------------------------------------------------------------------- F1: humidor

#[test]
fn a_humidor_entry_named_dot_does_not_take_the_whole_directory() {
    let fx = fx();
    let h = fx.home.humidor();
    fs::create_dir_all(h.join("1.20.0")).unwrap();
    fs::create_dir_all(h.join("9.9.9")).unwrap();
    fs::write(h.join("9.9.9/cig"), b"someone else's").unwrap();
    fs::write(h.join("1.20.0/cig"), b"ours").unwrap();
    let entry = |ver: &str, source: &str| {
        serde_json::json!({
            "version": ver, "source": source, "sha256": "0", "fetched_at": "x",
            "last_lit": null, "trust": "verified"
        })
    };
    let index = serde_json::json!({ "entries": [
        entry(".", "ciginstaller"),
        entry("..", "ciginstaller"),
        entry("a/b", "ciginstaller"),
        entry("1.20", "ciginstaller"), // parses, but is not how a version prints
        entry("1.20.0", "ciginstaller"),
        entry("9.9.9", "cig"),
    ]});
    fs::write(h.join("index.json"), index.to_string()).unwrap();
    let mut t = fx.txn("remove all mirrors");
    let gone = remove_all_humidor_mirrors(&mut t, &fx.home).unwrap();
    t.commit().unwrap();
    assert_eq!(gone, vec!["1.20.0".to_string()]);
    assert!(
        h.join("9.9.9/cig").exists(),
        "an entry that is not ours stays"
    );
    assert!(!h.join("1.20.0").exists());
    assert!(h.is_dir() && h.join("index.json").is_file());
    let left: serde_json::Value =
        serde_json::from_slice(&fs::read(h.join("index.json")).unwrap()).unwrap();
    // the odd ones are kept as they were, the foreign one too
    assert_eq!(left["entries"].as_array().unwrap().len(), 5);
}

// ------------------------------------------------------ F16: a profile that is a link

#[test]
fn a_profile_that_is_a_link_is_rewritten_never_deleted() {
    let fx = fx();
    let user = fx.home.user_home();
    let dotfiles = fx.tmp.path().join("dotfiles");
    fs::create_dir_all(&dotfiles).unwrap();
    let real = dotfiles.join("profile");
    let (profile, block) = path_block(Shell::Sh, &fx.home);
    // a profile that holds nothing but the block (written by hand into a dotfiles repository)
    fs::write(&real, &block).unwrap();
    fs::create_dir_all(&user).unwrap();
    symlink(&real, &profile).unwrap();
    assert_eq!(
        block_state(Shell::Sh, &fx.home).unwrap(),
        PathChange::AlreadyPresent
    );
    let mut t = fx.txn("remove block");
    let c = remove_path_block(&mut t, Shell::Sh, &fx.home).unwrap();
    t.commit().unwrap();
    assert_eq!(c, PathChange::Emptied);
    assert!(fs::symlink_metadata(&profile)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(fs::read(&real).unwrap(), b"", "emptied, not deleted");
}

// ------------------------------------------------------- F2 / F8: marker and modes

#[test]
fn the_layout_writes_an_owner_marker_and_tightens_the_home() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join(".cigscript");
    fs::create_dir_all(root.join("installer")).unwrap();
    for d in [&root, &root.join("installer")] {
        fs::set_permissions(d, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    }
    let home = Home::at(root.clone());
    assert_eq!(home.loose_dirs().len(), 2);
    home.ensure_layout().unwrap();
    assert!(home.loose_dirs().is_empty(), "{:?}", home.loose_dirs());
    let m = fs::read_to_string(home.owner_marker()).unwrap();
    assert!(m.starts_with("ciginstaller"), "{m}");
    // a marker that is there is not overwritten
    fs::write(home.owner_marker(), "something else\n").unwrap();
    home.ensure_layout().unwrap();
    assert_eq!(
        fs::read_to_string(home.owner_marker()).unwrap(),
        "something else\n"
    );
}
