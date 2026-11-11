// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! The move of an old home's `store/` under the humidor (ADR 0001 section 3), on scratch homes.

mod common;

use std::fs;

use cigi_home::migrate::migrate_in;
use cigi_home::*;
use common::*;

/// A home as the layout before the move made it: objects in `store/`, a toolchain whose files are
/// hard links to them, and cig's own humidor beside, with an entry the installer does not own.
fn old_home() -> Fx {
    let fx = fx();
    let mut t = fx.txn("install cigo");
    let files = fx.staged(&[
        ("bin/cigo", b"#!/bin/sh\necho cigo\n", true),
        ("orchestrate.cig", b"shared across versions", false),
    ]);
    materialize(&mut t, &fx.home, "cigo", &v("1.17.0"), &files, prov()).unwrap();
    t.commit().unwrap();
    let h = fx.home.humidor();
    fs::create_dir_all(h.join("1.19.0")).unwrap();
    fs::write(h.join("1.19.0").join("cig"), b"cig built by hand").unwrap();
    fs::write(
        h.join("index.json"),
        b"{\n  \"entries\": [{\"version\":\"1.19.0\",\"source\":\"local\"}]\n}\n",
    )
    .unwrap();
    // what a home made by the old layout looks like
    fs::rename(fx.home.shared_store(), fx.home.legacy_store()).unwrap();
    assert!(fx.home.is_legacy_store());
    fx
}

fn objects(h: &Home) -> Vec<(ObjectId, u64)> {
    h.object_store().all()
}

#[test]
fn an_old_home_reads_in_place_before_anything_moves() {
    let fx = old_home();
    let h = &fx.home;
    assert_eq!(h.store(), h.legacy_store());
    let seen = objects(h);
    assert!(seen.len() >= 2);
    // `ensure_layout` on an old home makes nothing under the humidor's store
    h.ensure_layout().unwrap();
    assert!(!h.shared_store().exists());
    assert_eq!(objects(h), seen);
}

#[test]
fn migration_moves_the_objects_under_the_humidor_and_is_idempotent() {
    let fx = old_home();
    let h = &fx.home;
    let before = objects(h);
    let hard_link = h.toolchain("cigo", &v("1.17.0")).join("orchestrate.cig");
    let m = migrate_store(h, &fx.lock).unwrap().expect("moved");
    assert_eq!(m.moved.objects as usize, before.len());
    assert!(!h.legacy_store().exists(), "the old store/ is gone");
    assert_eq!(h.store(), h.shared_store());
    assert_eq!(
        objects(h),
        before,
        "the same objects, found in the new place"
    );
    assert!(h.shared_store().join("tmp").is_dir());
    // the toolchain's file is still the object: a rename keeps the inode
    let id = before
        .iter()
        .map(|(i, _)| i)
        .find(|i| !i.exec && h.object_store().object_path(i).exists())
        .unwrap();
    assert!(platform::same_file(
        &hard_link,
        &h.object_store().object_path(id)
    ));
    // cig's own shelf is as it was, and what cig reads there is untouched
    assert_eq!(
        fs::read(h.humidor().join("1.19.0").join("cig")).unwrap(),
        b"cig built by hand"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: std::path::PathBuf| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(h.shared_store()), 0o700);
        assert_eq!(mode(h.shared_store().join("objects")), 0o700);
    }
    // a second run has nothing to do
    assert_eq!(migrate_store(h, &fx.lock).unwrap(), None);
    // and the journal says what was done
    let ops = fs::read_to_string(h.txn_root().join(&m.txn).join("journal.jsonl")).unwrap();
    for op in ["\"t\":\"move\"", "\"t\":\"delete\"", "\"t\":\"commit\""] {
        assert!(ops.contains(op), "{op} in {ops}");
    }
}

#[test]
fn a_rolled_back_migration_restores_the_old_home_byte_for_byte() {
    let fx = old_home();
    let h = &fx.home;
    let before = snapshot(&h.root());
    let objs = objects(h);

    let mut t = fx.txn("migrate store");
    migrate_in(&mut t, h).unwrap();
    assert_ne!(snapshot(&h.root()), before, "the move happened");
    assert!(!h.legacy_store().exists());
    let report = t.rollback().unwrap();
    assert!(report.is_complete(), "{:?}", report.failed);

    assert_eq!(snapshot(&h.root()), before);
    assert!(h.is_legacy_store());
    assert_eq!(objects(h), objs);
}

#[test]
fn a_migration_cut_short_is_undone_by_recover_and_a_read_in_between_sees_the_objects() {
    let fx = old_home();
    let h = &fx.home;
    let before = snapshot(&h.root());
    let objs = objects(h);

    // stop after `humidor/store/` is made and before anything is moved: the process died
    let mut t = fx.txn("migrate store");
    t.mkdir_all(&h.shared_store()).unwrap();
    drop(t);
    assert!(h.shared_store().is_dir());
    assert_eq!(objects(h), objs, "a reader still finds the objects");

    let r = recover(h, &fx.lock).unwrap();
    assert!(r.iter().all(|x| !x.is_problem()), "{r:?}");
    assert_eq!(snapshot(&h.root()), before);

    // a crash after the objects moved and before the old store/ went
    let mut t = fx.txn("migrate store");
    t.mkdir_all(&h.shared_store()).unwrap();
    t.move_path(
        &h.legacy_store().join("objects"),
        &h.shared_store().join("objects"),
    )
    .unwrap();
    drop(t);
    assert_eq!(
        objects(h),
        objs,
        "moved objects are read from the new place"
    );
    let r = recover(h, &fx.lock).unwrap();
    assert!(r.iter().all(|x| !x.is_problem()), "{r:?}");
    assert_eq!(snapshot(&h.root()), before);
    assert_eq!(objects(h), objs);
}

#[test]
fn a_new_home_is_made_with_the_store_under_the_humidor() {
    let fx = fx();
    let h = &fx.home;
    assert_eq!(h.store(), h.humidor().join("store"));
    assert!(h.humidor().join("store/objects").is_dir());
    assert!(h.humidor().join("store/tmp").is_dir());
    assert!(!h.legacy_store().exists());
    assert_eq!(migrate_store(h, &fx.lock).unwrap(), None);
}
