// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! Regression tests for the correctness review of phase 1 (scratch homes only).

mod common;
use cigi_home::*;
use common::*;
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

fn user_home(f: &Fx) -> (PathBuf, Home) {
    let user = f.tmp.path().join("u");
    (user.clone(), f.home.clone().with_user_home(user))
}

// ---------------------------------------------------------------- H1

#[test]
fn recover_never_removes_what_a_user_put_in_a_directory_the_txn_made() {
    let f = fx();
    let (user, home) = user_home(&f);
    let mut t = Txn::begin(&home, "install", &f.lock).unwrap();
    let (profile, _b) = path_block(Shell::Fish, &home);
    t.mkdir_all(profile.parent().unwrap()).unwrap();
    drop(t); // crash
    let cfg = user.join(".config/fish/config.fish");
    fs::write(&cfg, "set -x EDITOR vim\n").unwrap();
    let r = recover(&home, &f.lock).unwrap();
    assert_eq!(r.len(), 1);
    assert!(!r[0].is_problem(), "{:?}", r[0]);
    assert_eq!(fs::read_to_string(&cfg).unwrap(), "set -x EDITOR vim\n");
    assert!(
        r[0].notes.iter().any(|n| n.contains("left in place")),
        "{:?}",
        r[0].notes
    );
}

#[test]
fn recover_removes_a_directory_it_made_when_it_is_still_empty() {
    let f = fx();
    let (user, home) = user_home(&f);
    let mut t = Txn::begin(&home, "install", &f.lock).unwrap();
    t.mkdir_all(&user.join(".config/fish/conf.d")).unwrap();
    drop(t);
    let r = recover(&home, &f.lock).unwrap();
    assert!(!r[0].is_problem());
    assert!(r[0].notes.is_empty(), "{:?}", r[0].notes);
    assert!(!user.join(".config").exists());
}

#[test]
fn rollback_leaves_a_directory_that_is_no_longer_empty_with_a_note() {
    let f = fx();
    let (user, home) = user_home(&f);
    let mut t = Txn::begin(&home, "install", &f.lock).unwrap();
    t.mkdir_all(&user.join("Documents/PowerShell")).unwrap();
    fs::write(user.join("Documents/PowerShell/mine.ps1"), "x").unwrap();
    let rep = t.rollback().unwrap();
    assert!(rep.is_complete(), "{:?}", rep.failed);
    assert!(user.join("Documents/PowerShell/mine.ps1").is_file());
    assert_eq!(rep.notes.len(), 2, "{:?}", rep.notes); // both levels kept
}

#[test]
fn a_failed_install_still_cleans_its_partial_toolchain_tree() {
    let f = fx();
    let mut t = f.txn("i");
    let files = f.staged(&[("bin/cigo", b"x", true), ("share/doc/a.md", b"y", false)]);
    materialize(&mut t, &f.home, "cigo", &v("1.17.0"), &files, prov()).unwrap();
    let rep = t.rollback().unwrap();
    assert!(rep.is_complete());
    assert!(!f.home.toolchains().join("cigo/1.17.0").exists());
    assert!(fs::read_dir(f.home.toolchains().join("cigo"))
        .map(|d| d.count() == 0)
        .unwrap_or(true));
}

// ---------------------------------------------------------------- M2

// Unix only: a profile that is a symlink gets the block written in its target (Unix symlinks).
#[cfg(unix)]
#[test]
fn a_symlinked_profile_gets_the_block_in_its_target() {
    let f = fx();
    let (user, home) = user_home(&f);
    fs::create_dir_all(user.join("dotfiles")).unwrap();
    let target = user.join("dotfiles/profile");
    fs::write(&target, "alias a=b\n").unwrap();
    let link = user.join(".profile");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let before = fs::read_to_string(&target).unwrap();

    let mut t = Txn::begin(&home, "install", &f.lock).unwrap();
    assert_eq!(
        add_path_block(&mut t, Shell::Bash, &home).unwrap(),
        PathChange::Added
    );
    assert!(fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
    let (_, block) = path_block(Shell::Bash, &home);
    assert!(fs::read_to_string(&target).unwrap().contains(&block));
    assert_eq!(
        add_path_block(&mut t, Shell::Bash, &home).unwrap(),
        PathChange::AlreadyPresent
    );
    t.commit().unwrap();

    let mut t = Txn::begin(&home, "uninstall", &f.lock).unwrap();
    assert_eq!(
        remove_path_block(&mut t, Shell::Bash, &home).unwrap(),
        PathChange::Removed
    );
    t.commit().unwrap();
    assert_eq!(fs::read_to_string(&target).unwrap(), before);
    assert!(fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
}

// Unix only: rollback of a symlinked profile restores its target (Unix symlinks).
#[cfg(unix)]
#[test]
fn a_symlinked_profile_rolls_back_through_its_target() {
    let f = fx();
    let (user, home) = user_home(&f);
    fs::create_dir_all(user.join("dotfiles")).unwrap();
    let target = user.join("dotfiles/profile");
    fs::write(&target, "alias a=b\n").unwrap();
    std::os::unix::fs::symlink(&target, user.join(".profile")).unwrap();
    let mut t = Txn::begin(&home, "install", &f.lock).unwrap();
    add_path_block(&mut t, Shell::Bash, &home).unwrap();
    t.rollback().unwrap();
    assert_eq!(fs::read_to_string(&target).unwrap(), "alias a=b\n");
}

// Unix only: a dangling profile symlink is skipped without error (Unix symlinks).
#[cfg(unix)]
#[test]
fn a_dangling_profile_link_is_skipped_not_an_error() {
    let f = fx();
    let (user, home) = user_home(&f);
    std::os::unix::fs::symlink(user.join("nowhere"), user.join(".profile")).unwrap();
    let mut t = Txn::begin(&home, "install", &f.lock).unwrap();
    assert_eq!(
        add_path_block(&mut t, Shell::Bash, &home).unwrap(),
        PathChange::Skipped
    );
    assert_eq!(
        remove_path_block(&mut t, Shell::Bash, &home).unwrap(),
        PathChange::Skipped
    );
    assert_eq!(
        path::block_state(Shell::Bash, &home).unwrap(),
        PathChange::Skipped
    );
    t.commit().unwrap();
}

// ---------------------------------------------------------------- M3

fn many(f: &Fx, tag: &str, n: usize) -> Vec<StagedFile> {
    (0..n)
        .map(|i| StagedFile {
            path: format!("d{}/f{}", i % 50, i).into(),
            source: f.src(&format!("{tag}-{i}"), format!("{tag}-body-{i}").as_bytes()),
            exec: false,
        })
        .collect()
}

#[test]
fn rollback_reads_the_receipts_once_not_once_per_file() {
    // The old cost was files x installed files (about 2.7 s at 2000 in a
    // release build, four times worse per doubling). Linear is well under
    // the bound even in a debug build.
    let n = 2000;
    let f = fx();
    let mut t = f.txn("a");
    materialize(
        &mut t,
        &f.home,
        "cigo",
        &v("1.0.0"),
        &many(&f, "a", n),
        prov(),
    )
    .unwrap();
    t.commit().unwrap();
    let mut t = f.txn("b");
    materialize(
        &mut t,
        &f.home,
        "cigo",
        &v("1.1.0"),
        &many(&f, "b", n),
        prov(),
    )
    .unwrap();
    let t0 = std::time::Instant::now();
    let r = t.rollback().unwrap();
    let took = t0.elapsed();
    assert!(r.is_complete());
    assert!(took.as_secs() < 5, "rollback of {n} files took {took:?}");
    // and it is still correct: the first toolchain's objects stayed
    assert!(verify_toolchain(&f.home, "cigo", &v("1.0.0")).is_empty());
    assert_eq!(f.home.object_store().all().len(), n);
}

#[test]
fn rollback_keeps_the_objects_an_installed_toolchain_shares() {
    let f = fx();
    let mut t = f.txn("a");
    let a = f.staged(&[("bin/cigo", b"shared", true)]);
    materialize(&mut t, &f.home, "cigo", &v("1.0.0"), &a, prov()).unwrap();
    t.commit().unwrap();
    let mut t = f.txn("b");
    let b = f.staged(&[("bin/cigo", b"shared", true), ("x", b"only-new", false)]);
    materialize(&mut t, &f.home, "cigo", &v("1.1.0"), &b, prov()).unwrap();
    t.rollback().unwrap();
    assert!(verify_toolchain(&f.home, "cigo", &v("1.0.0")).is_empty());
    assert_eq!(f.home.object_store().all().len(), 1);
}

// ---------------------------------------------------------------- M5

#[test]
fn gc_keep_set_holds_the_objects_bin_links_to() {
    let f = fx();
    let mut t = f.txn("a");
    let shim_src = f.src("shim", b"SHIM");
    let obj = t
        .store_file(&f.home.object_store(), &shim_src, true)
        .unwrap();
    sync_shims(&mut t, &f.home, &["cig", "cigo"], &obj).unwrap();
    let files = f.staged(&[("bin/cigo", b"x-cigo", true)]);
    materialize(&mut t, &f.home, "cigo", &v("1.17.0"), &files, prov()).unwrap();
    t.commit().unwrap();
    let keep: HashSet<ObjectId> = toolchain::referenced_objects(&f.home).into_keys().collect();
    let un = f.home.object_store().unreferenced(&keep);
    assert!(un.is_empty(), "gc would delete {:?}", un);
}

#[test]
fn gc_keep_set_holds_a_toolchain_whose_receipt_is_unreadable() {
    let f = fx();
    let mut t = f.txn("a");
    let files = f.staged(&[("bin/cigo", b"x-cigo", true), ("doc/a", b"aa", false)]);
    materialize(&mut t, &f.home, "cigo", &v("1.17.0"), &files, prov()).unwrap();
    t.commit().unwrap();
    let orphan = f
        .home
        .object_store()
        .put_file(&f.src("orphan", b"orphan"), false)
        .unwrap();
    let rp = f
        .home
        .toolchain("cigo", &v("1.17.0"))
        .join(".cigi-receipt.json");
    let txt = fs::read_to_string(&rp).unwrap();
    fs::write(&rp, &txt[..txt.len() - 5]).unwrap(); // truncated
    let keep: HashSet<ObjectId> = toolchain::referenced_objects(&f.home).into_keys().collect();
    let un = f.home.object_store().unreferenced(&keep);
    assert_eq!(un.len(), 1, "{:?}", un);
    assert_eq!(un[0].0, orphan); // only the true orphan is collectable
}

// ---------------------------------------------------------------- L2

#[test]
fn a_failed_rebuild_leaves_the_live_toolchain_where_it_was() {
    let f = fx();
    let mut t = f.txn("a");
    let files = f.staged(&[("bin/cigo", b"x-cigo", true)]);
    materialize(&mut t, &f.home, "cigo", &v("1.17.0"), &files, prov()).unwrap();
    t.commit().unwrap();
    // damage it so a rebuild is due
    let dir = f.home.toolchain("cigo", &v("1.17.0"));
    fs::write(dir.join("stray"), "x").unwrap();
    // a rebuild whose second file cannot be staged
    let mut bad = f.staged(&[("bin/cigo", b"x-cigo", true)]);
    bad.push(StagedFile {
        path: "bin/missing".into(),
        source: f.tmp.path().join("stage/does-not-exist"),
        exec: false,
    });
    let mut t = f.txn("b");
    assert!(materialize(&mut t, &f.home, "cigo", &v("1.17.0"), &bad, prov()).is_err());
    assert!(
        dir.join("bin/cigo").is_file(),
        "the live toolchain was taken away first"
    );
    t.rollback().unwrap();
    assert!(dir.join("stray").is_file());
}

#[test]
fn a_rebuild_replaces_the_damaged_toolchain() {
    let f = fx();
    let mut t = f.txn("a");
    let files = f.staged(&[("bin/cigo", b"x-cigo", true)]);
    materialize(&mut t, &f.home, "cigo", &v("1.17.0"), &files, prov()).unwrap();
    t.commit().unwrap();
    let dir = f.home.toolchain("cigo", &v("1.17.0"));
    fs::write(dir.join("stray"), "x").unwrap();
    let mut t = f.txn("b");
    materialize(&mut t, &f.home, "cigo", &v("1.17.0"), &files, prov()).unwrap();
    t.commit().unwrap();
    assert!(!dir.join("stray").exists());
    assert!(verify_toolchain(&f.home, "cigo", &v("1.17.0")).is_empty());
}

// ---------------------------------------------------------------- L3

// Unix only: toolchain directories are never group-writable under umask 002 (Unix modes).
#[cfg(unix)]
#[test]
fn toolchain_directories_are_never_group_writable_under_umask_002() {
    use std::os::unix::fs::PermissionsExt;
    if std::env::var("CIGI_UMASK_CHILD").is_ok() {
        let f = fx();
        let mut t = f.txn("a");
        let files = f.staged(&[("bin/cigo", b"x", true), ("share/doc/a.md", b"y", false)]);
        materialize(&mut t, &f.home, "cigo", &v("1.17.0"), &files, prov()).unwrap();
        t.commit().unwrap();
        for p in [
            "toolchains/cigo",
            "toolchains/cigo/1.17.0",
            "toolchains/cigo/1.17.0/bin",
            "toolchains/cigo/1.17.0/share",
            "toolchains/cigo/1.17.0/share/doc",
        ] {
            let m = fs::metadata(f.home.root().join(p))
                .unwrap()
                .permissions()
                .mode()
                & 0o7777;
            assert_eq!(m, 0o755, "{p} is {m:o}");
        }
        f.home.check_permissions().unwrap();
        // and the check now looks that deep
        fs::set_permissions(
            f.home.toolchains().join("cigo/1.17.0/share"),
            fs::Permissions::from_mode(0o775),
        )
        .unwrap();
        assert_eq!(f.home.check_permissions().unwrap_err().code(), "E1835");
        return;
    }
    // run the body in a child whose umask is 002 (no unsafe, no libc)
    let exe = std::env::current_exe().unwrap();
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg("umask 002; exec \"$0\" --exact toolchain_directories_are_never_group_writable_under_umask_002 --nocapture")
        .arg(exe)
        .env("CIGI_UMASK_CHILD", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

// ---------------------------------------------------------------- L6

#[test]
fn removing_the_block_restores_an_empty_profile_byte_exact() {
    let f = fx();
    let (user, home) = user_home(&f);
    fs::write(user.join(".profile"), "").unwrap();
    let mut t = Txn::begin(&home, "i", &f.lock).unwrap();
    add_path_block(&mut t, Shell::Bash, &home).unwrap();
    t.commit().unwrap();
    let mut t = Txn::begin(&home, "u", &f.lock).unwrap();
    remove_path_block(&mut t, Shell::Bash, &home).unwrap();
    t.commit().unwrap();
    assert_eq!(fs::read(user.join(".profile")).unwrap(), b"");
}

#[test]
fn a_profile_the_add_created_is_deleted_again() {
    let f = fx();
    let (user, home) = user_home(&f);
    let mut t = Txn::begin(&home, "i", &f.lock).unwrap();
    add_path_block(&mut t, Shell::Bash, &home).unwrap();
    t.commit().unwrap();
    assert!(user.join(".profile").is_file());
    let mut t = Txn::begin(&home, "u", &f.lock).unwrap();
    remove_path_block(&mut t, Shell::Bash, &home).unwrap();
    t.commit().unwrap();
    assert!(!user.join(".profile").exists());
}

#[test]
fn removing_the_block_restores_a_crlf_profile_byte_exact() {
    let f = fx();
    let (user, home) = user_home(&f);
    let original = b"# mine\r\nexport A=1\r\n".to_vec();
    fs::write(user.join(".profile"), &original).unwrap();
    let mut t = Txn::begin(&home, "i", &f.lock).unwrap();
    add_path_block(&mut t, Shell::Bash, &home).unwrap();
    t.commit().unwrap();
    let added = fs::read(user.join(".profile")).unwrap();
    assert!(!String::from_utf8_lossy(&added)
        .replace("\r\n", "")
        .contains('\n'));
    // adding again is recognised, not doubled
    let mut t = Txn::begin(&home, "i2", &f.lock).unwrap();
    assert_eq!(
        add_path_block(&mut t, Shell::Bash, &home).unwrap(),
        PathChange::AlreadyPresent
    );
    t.commit().unwrap();
    let mut t = Txn::begin(&home, "u", &f.lock).unwrap();
    assert_eq!(
        remove_path_block(&mut t, Shell::Bash, &home).unwrap(),
        PathChange::Removed
    );
    t.commit().unwrap();
    assert_eq!(fs::read(user.join(".profile")).unwrap(), original);
}

// ---------------------------------------------------------------- L10

#[test]
fn recover_cleans_empty_and_torn_journal_directories() {
    let f = fx();
    let root = f.home.txn_root();
    let a = root.join("20260101T000000-aaaaaa");
    let b = root.join("20260101T000001-bbbbbb");
    let c = root.join("20260101T000002-cccccc");
    for d in [&a, &b, &c] {
        fs::create_dir_all(d.join("saved")).unwrap();
    }
    fs::write(b.join("journal.jsonl"), "").unwrap(); // created, never written
    fs::write(c.join("journal.jsonl"), "{\"seq\":0,\"op\":\"begi").unwrap(); // torn begin
    let r = recover(&f.home, &f.lock).unwrap();
    assert!(r.is_empty(), "{r:?}");
    assert!(!a.exists() && !b.exists() && !c.exists());
    assert!(list_txns(&f.home).is_empty());
}

#[test]
fn a_corrupt_journal_of_a_committed_txn_does_not_block_and_is_settled() {
    let f = fx();
    let mut t = f.txn("done");
    t.write(&f.home.root().join("x.txt"), b"x", None).unwrap();
    let dir = t.dir().to_path_buf();
    t.commit().unwrap();
    let j = dir.join("journal.jsonl");
    let mut text = fs::read_to_string(&j).unwrap();
    let good = text.clone();
    text.push_str("{this is not json}\n");
    fs::write(&j, text).unwrap();
    let r = recover(&f.home, &f.lock).unwrap();
    assert_eq!(r.len(), 1);
    assert!(!r[0].is_problem(), "{:?}", r[0]);
    assert!(matches!(r[0].outcome, RecoverOutcome::Settled { .. }));
    assert_eq!(fs::read_to_string(&j).unwrap(), good);
    // settled: the next run has nothing to say
    assert!(recover(&f.home, &f.lock).unwrap().is_empty());
}

#[test]
fn a_corrupt_journal_before_any_end_line_still_blocks() {
    let f = fx();
    let mut t = f.txn("open");
    t.write(&f.home.root().join("x.txt"), b"x", None).unwrap();
    let j = t.dir().join("journal.jsonl");
    drop(t);
    let mut text = fs::read_to_string(&j).unwrap();
    text.push_str("{this is not json}\n");
    fs::write(&j, text).unwrap();
    let r = recover(&f.home, &f.lock).unwrap();
    assert!(r[0].is_problem());
}

// ---------------------------------------------------------------- L4

#[test]
fn the_index_never_names_a_yanked_carton_for_a_series_pin() {
    let tool = |v: &str| {
        std::collections::BTreeMap::from([(
            "cigo".to_string(),
            PathBuf::from(format!("/h/toolchains/cigo/{v}/bin/cigo")),
        )])
    };
    let mut idx = ShimIndex::default();
    idx.cartons.insert("0.1.1".into(), tool("1.0.0"));
    idx.cartons.insert("0.1.2".into(), tool("1.0.1"));
    assert!(idx.render().contains("carton-minor\t0.1\t0.1.2\n"));
    assert_eq!(idx.pick(Some("0.1"), "cigo").unwrap().0.unwrap(), "0.1.2");
    let idx = idx.with_yanked(["0.1.2".to_string()]);
    let text = idx.render();
    assert!(text.contains("carton-minor\t0.1\t0.1.1\n"), "{text}");
    assert!(text.contains("carton-major\t0\t0.1.1\n"), "{text}");
    assert_eq!(idx.pick(Some("0.1"), "cigo").unwrap().0.unwrap(), "0.1.1");
    // an exact pin still names it: the resolver allows that and `check` warns
    assert_eq!(idx.pick(Some("0.1.2"), "cigo").unwrap().0.unwrap(), "0.1.2");
    // all of a series yanked: no line, so the shim goes to the resolver
    let idx = idx.with_yanked(["0.1.1".to_string(), "0.1.2".to_string()]);
    assert!(!idx.render().contains("carton-minor"));
    assert!(idx.pick(Some("0.1"), "cigo").is_none());
}

#[test]
fn the_reference_pick_has_the_shims_pin_grammar() {
    let mut idx = ShimIndex::default();
    idx.cartons.insert(
        "0.3.0-beta.1".into(),
        std::collections::BTreeMap::from([("cigo".to_string(), PathBuf::from("/h/x"))]),
    );
    assert!(idx.pick(Some("0.3.0-beta.1"), "cigo").is_none());
    assert!(idx.pick(Some("0.3."), "cigo").is_none());
}

// ---------------------------------------------------------------- L11

#[test]
fn two_sticks_of_one_tool_are_refused_not_last_wins() {
    let stick = |ver: &str| cigi_home::state::Stick {
        tool: "cigo".into(),
        version: ver.into(),
        from_carton: "0.0.9".into(),
        installed_at: "2026-11-12T10:03:11Z".into(),
        reason: "pin cigo".into(),
    };
    let mut s = State {
        sticks: vec![stick("1.16.0"), stick("1.17.0")],
        ..State::default()
    };
    let f = fx();
    let mut t = f.txn("s");
    let e = s.save_in(&mut t).unwrap_err();
    assert!(e.to_string().contains("two sticks of cigo"), "{e}");
    t.rollback().unwrap();
    // and a state.json that already has two is not read as if it had one
    let text = s.to_json();
    assert!(State::parse(&text)
        .unwrap_err()
        .contains("two sticks of cigo"));
    s.sticks.pop();
    assert!(State::parse(&s.to_json()).is_ok());
}
