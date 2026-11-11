// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! Store, toolchain, journal-op, shim, PATH, humidor, state and config tests
//! through the public API, on scratch homes.

mod common;

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use cigi_home::*;
use common::*;

// ------------------------------------------------------------ materialize

fn cigo_files(fx: &Fx, extra: &[u8]) -> Vec<StagedFile> {
    fx.staged(&[
        ("bin/cigo", b"#!/bin/sh\necho cigo\n", true),
        ("orchestrate.cig", b"shared across versions", false),
        ("docs/a.md", extra, false),
    ])
}

fn install_cigo(fx: &Fx, ver: &str, extra: &[u8]) -> Receipt {
    let mut t = fx.txn("install cigo");
    let files = cigo_files(fx, extra);
    let r = materialize(&mut t, &fx.home, "cigo", &v(ver), &files, prov()).unwrap();
    t.commit().unwrap();
    r
}

#[test]
fn materialize_builds_a_verified_toolchain_with_a_receipt() {
    let fx = fx();
    let r = install_cigo(&fx, "1.17.0", b"docs");
    assert_eq!(r.format, 1);
    assert_eq!(r.tool, "cigo");
    assert_eq!(r.version, "1.17.0");
    assert_eq!(
        r.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
        ["bin/cigo", "docs/a.md", "orchestrate.cig"]
    );
    assert!(r.files[0].exec && !r.files[1].exec);
    assert_eq!(r.shim_target("cigo"), "bin/cigo");
    assert!(verify_toolchain(&fx.home, "cigo", &v("1.17.0")).is_empty());
    assert_eq!(installed(&fx.home), vec![("cigo".to_string(), v("1.17.0"))]);
    // receipt on disk equals what was returned and is the documented shape
    let text = fs::read_to_string(
        fx.home
            .toolchain("cigo", &v("1.17.0"))
            .join(".cigi-receipt.json"),
    )
    .unwrap();
    assert!(text.starts_with("{\n  \"format\": 1,\n  \"tool\": \"cigo\","));
    assert!(text.ends_with("}\n"));
    assert_eq!(read_receipt(&fx.home, "cigo", &v("1.17.0")).unwrap(), r);
    // no partial directory is left
    let names: Vec<_> = fs::read_dir(fx.home.toolchains().join("cigo"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["1.17.0"]);
}

#[test]
fn toolchain_files_are_read_only_hard_links_of_store_objects() {
    let fx = fx();
    let r = install_cigo(&fx, "1.17.0", b"docs");
    let tc = fx.home.toolchain("cigo", &v("1.17.0"));
    for f in &r.files {
        let id = ObjectId::new(f.sha256.clone(), f.exec);
        let obj = fx.home.object_store().object_path(&id);
        let p = tc.join(&f.path);
        if let (Some(a), Some(b)) = (platform::ino(&p), platform::ino(&obj)) {
            assert_eq!(a, b, "{} is a link of its object", f.path);
        }
        if let Some(m) = platform::mode_of(&p) {
            assert_eq!(m, if f.exec { 0o555 } else { 0o444 }, "{}", f.path);
        }
    }
}

#[test]
fn two_versions_share_objects_for_common_files() {
    let fx = fx();
    install_cigo(&fx, "1.17.0", b"docs v1");
    install_cigo(&fx, "1.17.1", b"docs v2");
    let a = fx
        .home
        .toolchain("cigo", &v("1.17.0"))
        .join("orchestrate.cig");
    let b = fx
        .home
        .toolchain("cigo", &v("1.17.1"))
        .join("orchestrate.cig");
    if let (Some(x), Some(y)) = (platform::ino(&a), platform::ino(&b)) {
        assert_eq!(x, y, "the common file is one inode");
    }
    // 2 shared (bin/cigo exec, orchestrate.cig) + 2 docs = 4 objects, not 6
    assert_eq!(fx.home.object_store().du().objects, 4);
}

#[test]
fn materialize_twice_is_a_no_op() {
    let fx = fx();
    let r1 = install_cigo(&fx, "1.17.0", b"docs");
    let before = snapshot(&fx.home.root());
    let r2 = install_cigo(&fx, "1.17.0", b"docs");
    assert_eq!(r1, r2, "the existing receipt comes back untouched");
    assert_eq!(snapshot(&fx.home.root()), before);
}

#[test]
fn one_corrupt_byte_is_found_missing_and_extra_too() {
    let fx = fx();
    install_cigo(&fx, "1.17.0", b"docs");
    let tc = fx.home.toolchain("cigo", &v("1.17.0"));
    // replace one file by rename (breaks only that link), as a tool would
    let p = tc.join("docs/a.md");
    fs::remove_file(&p).unwrap();
    fs::write(&p, b"dxcs").unwrap();
    fs::remove_file(tc.join("orchestrate.cig")).unwrap();
    fs::write(tc.join("stray.txt"), b"?").unwrap();
    let d = verify_toolchain(&fx.home, "cigo", &v("1.17.0"));
    assert!(
        d.iter()
            .any(|x| matches!(x, Damage::Changed { path, .. } if path == "docs/a.md")),
        "{d:?}"
    );
    assert!(
        d.iter()
            .any(|x| matches!(x, Damage::Missing { path } if path == "orchestrate.cig")),
        "{d:?}"
    );
    assert!(
        d.iter()
            .any(|x| matches!(x, Damage::Extra { path } if path == "stray.txt")),
        "{d:?}"
    );
    assert_eq!(d.len(), 3);
    // the store object stayed intact
    assert!(fx.home.object_store().all().iter().all(|(id, _)| fx
        .home
        .object_store()
        .verify(id)
        .is_ok()));
}

#[test]
fn repair_restores_from_the_store_and_rolls_back_cleanly() {
    let fx = fx();
    let r = install_cigo(&fx, "1.17.0", b"docs");
    let tc = fx.home.toolchain("cigo", &v("1.17.0"));
    let p = tc.join("docs/a.md");
    fs::remove_file(&p).unwrap();
    fs::write(&p, b"dxcs").unwrap();
    fs::write(tc.join("stray.txt"), b"?").unwrap();
    let damaged = snapshot(&fx.home.root());

    // a repair that is rolled back leaves the damage exactly as found
    let mut t = fx.txn("repair");
    let f = r.files.iter().find(|f| f.path == "docs/a.md").unwrap();
    toolchain::restore_file(&mut t, &fx.home, "cigo", &v("1.17.0"), f).unwrap();
    toolchain::remove_extra(&mut t, &fx.home, "cigo", &v("1.17.0"), "stray.txt").unwrap();
    assert!(verify_toolchain(&fx.home, "cigo", &v("1.17.0")).is_empty());
    assert!(t.rollback().unwrap().is_complete());
    assert_eq!(snapshot(&fx.home.root()), damaged);
    assert_eq!(fs::read(&p).unwrap(), b"dxcs");

    let mut t = fx.txn("repair");
    toolchain::restore_file(&mut t, &fx.home, "cigo", &v("1.17.0"), f).unwrap();
    toolchain::remove_extra(&mut t, &fx.home, "cigo", &v("1.17.0"), "stray.txt").unwrap();
    t.commit().unwrap();
    assert!(verify_toolchain(&fx.home, "cigo", &v("1.17.0")).is_empty());
}

#[test]
fn a_toolchain_without_a_receipt_is_not_trusted_and_gets_rebuilt() {
    let fx = fx();
    let dir = fx.home.toolchain("cigo", &v("1.0.0"));
    fs::create_dir_all(dir.join("bin")).unwrap();
    fs::write(dir.join("bin/cigo"), b"old").unwrap();
    assert!(installed(&fx.home).is_empty());
    assert_eq!(
        verify_toolchain(&fx.home, "cigo", &v("1.0.0")),
        vec![Damage::NoReceipt]
    );
    let before = snapshot(&fx.home.root());
    let mut t = fx.txn("install");
    let files = cigo_files(&fx, b"d");
    materialize(&mut t, &fx.home, "cigo", &v("1.0.0"), &files, prov()).unwrap();
    assert!(verify_toolchain(&fx.home, "cigo", &v("1.0.0")).is_empty());
    // the old tree comes back byte for byte on rollback
    assert!(t.rollback().unwrap().is_complete());
    assert_eq!(snapshot(&fx.home.root()), before);
    assert_eq!(fs::read(dir.join("bin/cigo")).unwrap(), b"old");
}

#[test]
fn unsafe_paths_are_refused_before_anything_is_touched() {
    let fx = fx();
    for bad in ["../x", "/abs", "a/../b", "", ".cigi-receipt.json", "./"] {
        let before = snapshot(&fx.home.root());
        let mut t = fx.txn("install");
        let files = vec![StagedFile {
            path: bad.into(),
            source: fx.src("f", b"x"),
            exec: false,
        }];
        let e = materialize(&mut t, &fx.home, "cigo", &v("1.0.0"), &files, prov()).unwrap_err();
        assert_eq!(e.code(), "E1890", "{bad}");
        t.rollback().unwrap();
        assert_eq!(snapshot(&fx.home.root()), before, "{bad}");
    }
    let mut t = fx.txn("install");
    let files = vec![
        StagedFile {
            path: "a".into(),
            source: fx.src("f", b"x"),
            exec: false,
        },
        StagedFile {
            path: "a".into(),
            source: fx.src("f", b"x"),
            exec: true,
        },
    ];
    assert!(materialize(&mut t, &fx.home, "cigo", &v("1.0.0"), &files, prov()).is_err());
}

#[test]
fn rollback_of_a_fresh_install_leaves_the_home_as_it_was() {
    let fx = fx();
    let before = snapshot(&fx.home.root());
    let mut t = fx.txn("install");
    let files = cigo_files(&fx, b"docs");
    materialize(&mut t, &fx.home, "cigo", &v("1.17.0"), &files, prov()).unwrap();
    assert_eq!(installed(&fx.home).len(), 1);
    let rep = t.rollback().unwrap();
    assert!(rep.is_complete());
    assert_eq!(rep.txn.len(), 22);
    assert!(installed(&fx.home).is_empty());
    assert_eq!(
        fx.home.object_store().du().objects,
        0,
        "objects this txn added are gone"
    );
    assert_eq!(snapshot(&fx.home.root()), before);
}

#[test]
fn rollback_keeps_objects_another_toolchain_uses() {
    let fx = fx();
    install_cigo(&fx, "1.17.0", b"docs v1");
    let objects_before = fx.home.object_store().all();
    let mut t = fx.txn("install");
    let files = cigo_files(&fx, b"docs v2");
    materialize(&mut t, &fx.home, "cigo", &v("1.17.1"), &files, prov()).unwrap();
    t.rollback().unwrap();
    assert_eq!(fx.home.object_store().all(), objects_before);
    assert!(verify_toolchain(&fx.home, "cigo", &v("1.17.0")).is_empty());
}

#[test]
fn crash_in_the_middle_of_materialize_is_recovered() {
    let fx = fx();
    install_cigo(&fx, "1.17.0", b"docs v1");
    let before = snapshot(&fx.home.root());
    {
        let mut t = fx.txn("install");
        let files = cigo_files(&fx, b"docs v2");
        materialize(&mut t, &fx.home, "cigo", &v("1.17.1"), &files, prov()).unwrap();
        // died here: no flip, no commit
    }
    // also a stray staging file and a half-built partial directory
    fs::write(fx.home.store().join("tmp/leftover.tmp"), b"x").unwrap();
    fs::create_dir_all(fx.home.toolchains().join("cigo/.9.9.9.partial-zzz/bin")).unwrap();
    let r = recover(&fx.home, &fx.lock).unwrap();
    assert_eq!(r.len(), 1);
    assert!(matches!(r[0].outcome, RecoverOutcome::RolledBack { .. }));
    assert_eq!(snapshot(&fx.home.root()), before);
}

#[test]
fn many_files_materialize_quickly() {
    let fx = fx();
    let n = 400usize;
    let bodies: Vec<Vec<u8>> = (0..n)
        .map(|i| format!("file number {i}").into_bytes())
        .collect();
    let files: Vec<StagedFile> = bodies
        .iter()
        .enumerate()
        .map(|(i, b)| StagedFile {
            path: format!("d{}/f{i}.txt", i % 20).into(),
            source: fx.src(&format!("m/{i}"), b),
            exec: false,
        })
        .collect();
    let started = std::time::Instant::now();
    let mut t = fx.txn("big");
    materialize(&mut t, &fx.home, "big", &v("1.0.0"), &files, prov()).unwrap();
    t.commit().unwrap();
    let took = started.elapsed();
    eprintln!("materialize of {n} files: {took:?}");
    assert!(took.as_secs() < 30, "{took:?}");
    assert!(verify_toolchain(&fx.home, "big", &v("1.0.0")).is_empty());
}

// ------------------------------------------------------------ journal ops

#[test]
fn every_op_kind_rolls_back_exactly() {
    let fx = fx();
    let h = &fx.home;
    // fixtures that exist before the transaction
    let keep = h.root().join("keep.txt");
    fs::write(&keep, b"original").unwrap();
    let doomed = h.root().join("doomed.txt");
    fs::write(&doomed, b"doomed").unwrap();
    let doomed_dir = h.root().join("doomed-dir");
    fs::create_dir_all(doomed_dir.join("sub")).unwrap();
    fs::write(doomed_dir.join("sub/f"), b"inner").unwrap();
    let linked = h.root().join("linked");
    fs::write(&linked, b"old link content").unwrap();
    let before = snapshot(&h.root());

    let mut t = fx.txn("everything");
    t.mkdir_all(&h.root().join("a/b/c")).unwrap(); // mkdir
    let obj = t
        .store_file(&h.object_store(), &fx.src("o", b"object"), true)
        .unwrap(); // store
    t.link(
        &h.object_store().object_path(&obj),
        &h.root().join("a/b/c/link"),
    )
    .unwrap(); // link (absent)
    t.link(&h.object_store().object_path(&obj), &linked)
        .unwrap(); // link (replace)
    t.write(&keep, b"rewritten", None).unwrap(); // write (existing)
    t.write(&h.root().join("a/new.txt"), b"fresh", None)
        .unwrap(); // write (absent)
    t.write_block(&keep, "cigscript", b"rewritten again", None)
        .unwrap(); // block
    t.delete(&doomed).unwrap(); // delete file
    t.delete(&doomed_dir).unwrap(); // delete dir
    t.move_path(&h.root().join("a/new.txt"), &h.root().join("a/moved.txt"))
        .unwrap(); // move
    assert!(!doomed.exists() && !doomed_dir.exists());
    assert_eq!(fs::read(&keep).unwrap(), b"rewritten again");

    let rep = t.rollback().unwrap();
    assert!(rep.is_complete(), "{:?}", rep.failed);
    assert_eq!(snapshot(&h.root()), before);
    assert_eq!(fs::read(&keep).unwrap(), b"original");
    assert_eq!(fs::read(&linked).unwrap(), b"old link content");
    assert_eq!(fs::read(doomed_dir.join("sub/f")).unwrap(), b"inner");
}

#[test]
fn rollback_is_idempotent_across_a_crash_in_the_middle_of_rollback() {
    let fx = fx();
    let h = &fx.home;
    let keep = h.root().join("keep.txt");
    fs::write(&keep, b"original").unwrap();
    let before = snapshot(&h.root());
    let id;
    {
        let mut t = fx.txn("x");
        id = t.id().to_string();
        t.write(&keep, b"changed", None).unwrap();
        t.delete(&keep).unwrap();
        t.mkdir(&h.root().join("newdir")).unwrap();
    }
    // a partial rollback by hand: the dir is gone, the rest is not yet
    fs::remove_dir(h.root().join("newdir")).unwrap();
    let r = recover(h, &fx.lock).unwrap();
    assert_eq!(r[0].txn, id);
    assert!(matches!(r[0].outcome, RecoverOutcome::RolledBack { .. }));
    assert_eq!(snapshot(&h.root()), before);
    assert_eq!(fs::read(&keep).unwrap(), b"original");
}

#[test]
fn identical_writes_record_nothing() {
    let fx = fx();
    let p = fx.home.root().join("same");
    fs::write(&p, b"same").unwrap();
    let t = &mut fx.txn("x");
    t.write(&p, b"same", None).unwrap();
    let text = fs::read_to_string(t.dir().join("journal.jsonl")).unwrap();
    assert_eq!(text.lines().count(), 1, "only `begin`");
}

#[test]
fn a_damaged_saved_copy_is_reported_not_trusted() {
    let fx = fx();
    let keep = fx.home.root().join("keep.txt");
    fs::write(&keep, b"original").unwrap();
    let mut t = fx.txn("x");
    t.write(&keep, b"changed", None).unwrap();
    let saved = t.dir().join("saved/0001");
    let dir = t.dir().to_path_buf();
    // someone edits the saved copy in place
    platform::set_mode(&saved, 0o644).unwrap();
    fs::write(&saved, b"tampered").unwrap();
    let rep = t.rollback().unwrap();
    assert!(!rep.is_complete());
    assert_eq!(rep.failed[0].what, "write");
    // no rollback-done: the next recover reports E1833 again
    let r = recover(&fx.home, &fx.lock).unwrap();
    assert_eq!(r[0].code(), "E1833");
    assert!(dir.exists());
}

// ------------------------------------------------------------------ shims

#[test]
fn shims_are_links_of_the_object_and_sync_is_idempotent() {
    let fx = fx();
    let mut t = fx.txn("shims");
    let id = t
        .store_file(
            &fx.home.object_store(),
            &fx.src("shim", b"\x7fELF shim"),
            true,
        )
        .unwrap();
    sync_shims(
        &mut t,
        &fx.home,
        &["cig", "cigo", "cigcompile", "cigar"],
        &id,
    )
    .unwrap();
    t.commit().unwrap();
    let obj = fx.home.object_store().object_path(&id);
    for n in ["cig", "cigo", "cigcompile", "cigar"] {
        let p = shim_path(&fx.home, n);
        assert!(p.is_file());
        if let (Some(a), Some(b)) = (platform::ino(&p), platform::ino(&obj)) {
            assert_eq!(a, b);
        }
    }
    assert_eq!(
        shims::shims_present(&fx.home, &id, &["cig", "nope"]),
        vec!["cig".to_string()]
    );
    let before = snapshot(&fx.home.root());
    let mut t = fx.txn("shims again");
    sync_shims(&mut t, &fx.home, &["cig", "cigo"], &id).unwrap();
    t.commit().unwrap();
    assert_eq!(snapshot(&fx.home.root()), before);
}

#[test]
fn a_new_shim_replaces_the_old_one_and_rollback_puts_it_back() {
    let fx = fx();
    let mut t = fx.txn("shims");
    let old = t
        .store_file(&fx.home.object_store(), &fx.src("s1", b"old shim"), true)
        .unwrap();
    sync_shims(&mut t, &fx.home, &["cig"], &old).unwrap();
    t.commit().unwrap();
    let mut t = fx.txn("shims v2");
    let new = t
        .store_file(&fx.home.object_store(), &fx.src("s2", b"new shim!"), true)
        .unwrap();
    sync_shims(&mut t, &fx.home, &["cig"], &new).unwrap();
    assert_eq!(fs::read(shim_path(&fx.home, "cig")).unwrap(), b"new shim!");
    t.rollback().unwrap();
    assert_eq!(fs::read(shim_path(&fx.home, "cig")).unwrap(), b"old shim");
    // the old object is the one linked again
    assert_eq!(
        platform::ino(&shim_path(&fx.home, "cig")),
        platform::ino(&fx.home.object_store().object_path(&old))
    );
}

#[test]
fn removing_a_shim_is_journaled() {
    let fx = fx();
    let mut t = fx.txn("s");
    let id = t
        .store_file(&fx.home.object_store(), &fx.src("s", b"shim"), true)
        .unwrap();
    sync_shims(&mut t, &fx.home, &["cig"], &id).unwrap();
    t.commit().unwrap();
    let mut t = fx.txn("rm");
    remove_shims(&mut t, &fx.home, &["cig", "absent"]).unwrap();
    assert!(!shim_path(&fx.home, "cig").exists());
    t.rollback().unwrap();
    assert!(shim_path(&fx.home, "cig").exists());
}

// -------------------------------------------------------------- PATH block

fn profile_fx() -> (Fx, std::path::PathBuf) {
    let fx = fx();
    let (profile, _) = path_block(Shell::Bash, &fx.home);
    (fx, profile)
}

#[test]
fn path_block_text_and_locations() {
    let fx = fx();
    let (p, b) = path_block(Shell::Bash, &fx.home);
    assert_eq!(p, fx.home.user_home().join(".profile"));
    assert_eq!(
        b,
        "# >>> cigscript >>>\nexport PATH=\"$HOME/.cigscript/bin:$PATH\"\n# <<< cigscript <<<\n"
    );
    assert_eq!(path_block(Shell::Sh, &fx.home).0, p);
    assert_eq!(
        path_block(Shell::Zsh, &fx.home).0,
        fx.home.user_home().join(".zprofile")
    );
    let (f, fb) = path_block(Shell::Fish, &fx.home);
    assert!(f.ends_with(".config/fish/conf.d/cigscript.fish"));
    assert!(fb.contains("fish_add_path \"$HOME/.cigscript/bin\""));
    assert!(path_block(Shell::PowerShell, &fx.home)
        .1
        .contains("$env:PATH"));
    // a home that is not ~/.cigscript gets the absolute path
    let custom = Home::at(fx.tmp.path().join("elsewhere")).with_user_home(fx.tmp.path().join("u"));
    let (_, cb) = path_block(Shell::Bash, &custom);
    assert!(
        cb.contains(&format!(
            "export PATH=\"{}/elsewhere/bin:$PATH\"",
            fx.tmp.path().display()
        )),
        "{cb}"
    );
    assert_eq!(Shell::from_program("/usr/bin/zsh"), Some(Shell::Zsh));
    assert_eq!(Shell::from_program("pwsh.exe"), Some(Shell::PowerShell));
    assert_eq!(Shell::from_program("tcsh"), None);
}

#[test]
fn add_is_idempotent_and_remove_restores_the_file_byte_for_byte() {
    for original in [
        &b"# my profile\nexport FOO=1\n"[..],
        b"",
        b"line\n\n\n",
        b"\n",
    ] {
        let (fx, profile) = profile_fx();
        if !original.is_empty() {
            fs::create_dir_all(profile.parent().unwrap()).unwrap();
            fs::write(&profile, original).unwrap();
            if platform::mode_of(&profile).is_some() {
                platform::set_mode(&profile, 0o600).unwrap();
            }
        }
        let mut t = fx.txn("path");
        assert_eq!(
            add_path_block(&mut t, Shell::Bash, &fx.home).unwrap(),
            PathChange::Added
        );
        assert_eq!(
            add_path_block(&mut t, Shell::Bash, &fx.home).unwrap(),
            PathChange::AlreadyPresent
        );
        let (_, block) = path_block(Shell::Bash, &fx.home);
        let text = fs::read_to_string(&profile).unwrap();
        assert!(text.starts_with(std::str::from_utf8(original).unwrap()));
        assert_eq!(text.matches("# >>> cigscript >>>").count(), 1);
        assert!(text.ends_with(&block));
        if !original.is_empty() {
            assert_eq!(
                platform::mode_of(&profile).map(|m| m & 0o777),
                platform::mode_of(&profile).map(|_| 0o600),
                "the profile keeps its mode"
            );
        }
        t.commit().unwrap();

        let mut t = fx.txn("unpath");
        assert_eq!(
            remove_path_block(&mut t, Shell::Bash, &fx.home).unwrap(),
            PathChange::Removed
        );
        assert_eq!(
            remove_path_block(&mut t, Shell::Bash, &fx.home).unwrap(),
            PathChange::NotPresent
        );
        t.commit().unwrap();
        if original.is_empty() {
            assert!(
                !profile.exists(),
                "a profile that held only the block is removed"
            );
        } else {
            assert_eq!(fs::read(&profile).unwrap(), original);
        }
    }
}

#[test]
fn rollback_restores_the_profile_exactly_even_without_a_trailing_newline() {
    let (fx, profile) = profile_fx();
    fs::create_dir_all(profile.parent().unwrap()).unwrap();
    fs::write(&profile, b"no newline at end").unwrap();
    let mut t = fx.txn("path");
    add_path_block(&mut t, Shell::Bash, &fx.home).unwrap();
    t.rollback().unwrap();
    assert_eq!(fs::read(&profile).unwrap(), b"no newline at end");
}

#[test]
fn a_hand_edited_block_is_left_alone() {
    let (fx, profile) = profile_fx();
    fs::create_dir_all(profile.parent().unwrap()).unwrap();
    let edited = "# >>> cigscript >>>\nexport PATH=\"/my/own:$PATH\"\n# <<< cigscript <<<\n";
    fs::write(&profile, edited).unwrap();
    let mut t = fx.txn("path");
    assert_eq!(
        add_path_block(&mut t, Shell::Bash, &fx.home).unwrap(),
        PathChange::Edited
    );
    assert_eq!(
        remove_path_block(&mut t, Shell::Bash, &fx.home).unwrap(),
        PathChange::Edited
    );
    assert_eq!(
        path::block_state(Shell::Bash, &fx.home).unwrap(),
        PathChange::Edited
    );
    assert_eq!(fs::read_to_string(&profile).unwrap(), edited);
    fs::write(&profile, "# >>> cigscript >>>\nhalf a block\n").unwrap();
    assert_eq!(
        add_path_block(&mut t, Shell::Bash, &fx.home).unwrap(),
        PathChange::Edited
    );
}

#[test]
fn fish_profile_is_created_with_its_directories_and_rolled_back_whole() {
    let fx = fx();
    let user = fx.home.user_home();
    assert!(!user.join(".config").exists());
    let mut t = fx.txn("path");
    assert_eq!(
        add_path_block(&mut t, Shell::Fish, &fx.home).unwrap(),
        PathChange::Added
    );
    let (p, _) = path_block(Shell::Fish, &fx.home);
    assert!(p.is_file());
    t.rollback().unwrap();
    assert!(
        !user.join(".config").exists(),
        "the directories it made are gone too"
    );
    assert!(!p.exists());
}

// ---------------------------------------------------------------- humidor

fn install_cig(fx: &Fx, ver: &str, level: &str) -> std::path::PathBuf {
    let mut t = fx.txn("install cig");
    let mut pr = prov();
    pr.trust.level = level.into();
    pr.shims = vec![ShimRef {
        name: "cig".into(),
        path: "bin/cig".into(),
    }];
    let files = fx.staged(&[("bin/cig", format!("cig binary {ver}").as_bytes(), true)]);
    materialize(&mut t, &fx.home, "cig", &v(ver), &files, pr).unwrap();
    let bin = fx.home.toolchain("cig", &v(ver)).join("bin/cig");
    assert_eq!(
        mirror_humidor(&mut t, &fx.home, &v(ver), &bin).unwrap(),
        HumidorMirror::Mirrored
    );
    t.commit().unwrap();
    bin
}

#[test]
fn humidor_mirror_links_the_binary_and_writes_cigs_index_format() {
    let fx = fx();
    let bin = install_cig(&fx, "1.20.0", "checksummed");
    let target = fx.home.humidor().join("1.20.0/cig");
    assert!(target.is_file());
    assert_eq!(platform::ino(&target), platform::ino(&bin));
    let text = fs::read_to_string(fx.home.humidor().join("index.json")).unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    let e = &json["entries"][0];
    assert_eq!(e["version"], "1.20.0");
    assert_eq!(e["source"], "ciginstaller");
    assert_eq!(e["trust"], "verified");
    assert_eq!(e["last_lit"], serde_json::Value::Null);
    assert_eq!(e["sha256"].as_str().unwrap().len(), 64);
    assert!(
        text.find("\"version\"").unwrap() < text.find("\"trust\"").unwrap(),
        "documented key order"
    );
    assert_eq!(
        read_receipt(&fx.home, "cig", &v("1.20.0"))
            .unwrap()
            .humidor_mirror
            .as_deref(),
        Some("mirrored")
    );
    // the toolchain still verifies: the receipt was rewritten, not counted as extra
    assert!(verify_toolchain(&fx.home, "cig", &v("1.20.0")).is_empty());
    assert_eq!(humidor::entries(&fx.home).unwrap().len(), 1);
}

#[test]
fn attested_artifacts_are_marked_attested() {
    let fx = fx();
    install_cig(&fx, "1.20.0", "attested");
    assert_eq!(humidor::entries(&fx.home).unwrap()[0].trust, "attested");
}

#[test]
fn mirroring_again_changes_nothing_and_keeps_other_entries() {
    let fx = fx();
    fs::create_dir_all(fx.home.humidor()).unwrap();
    fs::write(
        fx.home.humidor().join("index.json"),
        "{\"entries\":[{\"version\":\"1.18.0\",\"source\":\"local\",\"sha256\":\"\",\"fetched_at\":\"\",\"last_lit\":null,\"trust\":\"unverified\"}],\"note\":\"mine\"}",
    )
    .unwrap();
    let bin = install_cig(&fx, "1.20.0", "checksummed");
    let first = fs::read_to_string(fx.home.humidor().join("index.json")).unwrap();
    let mut t = fx.txn("again");
    assert_eq!(
        mirror_humidor(&mut t, &fx.home, &v("1.20.0"), &bin).unwrap(),
        HumidorMirror::Mirrored
    );
    t.commit().unwrap();
    assert_eq!(
        fs::read_to_string(fx.home.humidor().join("index.json")).unwrap(),
        first
    );
    let json: serde_json::Value = serde_json::from_str(&first).unwrap();
    assert_eq!(json["entries"].as_array().unwrap().len(), 2);
    assert_eq!(json["entries"][0]["version"], "1.18.0");
    assert_eq!(json["note"], "mine");
}

#[test]
fn a_foreign_humidor_entry_is_skipped_and_recorded() {
    let fx = fx();
    let dir = fx.home.humidor().join("1.20.0");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("cig"), b"their cig").unwrap();
    fs::write(
        fx.home.humidor().join("index.json"),
        "{\"entries\":[{\"version\":\"1.20.0\",\"source\":\"v1.20.0\",\"sha256\":\"abc\",\"fetched_at\":\"x\",\"last_lit\":null,\"trust\":\"attested\"}]}",
    )
    .unwrap();
    let index_before = fs::read(fx.home.humidor().join("index.json")).unwrap();
    let mut t = fx.txn("cig");
    let files = fx.staged(&[("bin/cig", b"mine", true)]);
    materialize(&mut t, &fx.home, "cig", &v("1.20.0"), &files, prov()).unwrap();
    let bin = fx.home.toolchain("cig", &v("1.20.0")).join("bin/cig");
    assert_eq!(
        mirror_humidor(&mut t, &fx.home, &v("1.20.0"), &bin).unwrap(),
        HumidorMirror::Skipped
    );
    t.commit().unwrap();
    assert_eq!(fs::read(dir.join("cig")).unwrap(), b"their cig");
    assert_eq!(
        fs::read(fx.home.humidor().join("index.json")).unwrap(),
        index_before
    );
    assert_eq!(
        read_receipt(&fx.home, "cig", &v("1.20.0"))
            .unwrap()
            .humidor_mirror
            .as_deref(),
        Some("skipped")
    );
    // a binary with no entry at all is also a hands-off
    let d2 = fx.home.humidor().join("1.19.0");
    fs::create_dir_all(&d2).unwrap();
    fs::write(d2.join("cig"), b"by hand").unwrap();
    let mut t = fx.txn("cig2");
    assert_eq!(
        mirror_humidor(&mut t, &fx.home, &v("1.19.0"), &bin).unwrap(),
        HumidorMirror::Skipped
    );
}

#[test]
fn removing_a_mirror_takes_the_link_and_the_entry_and_rolls_back() {
    let fx = fx();
    install_cig(&fx, "1.20.0", "checksummed");
    let before = snapshot(&fx.home.root());
    let mut t = fx.txn("rm");
    assert!(remove_humidor_mirror(&mut t, &fx.home, &v("1.20.0")).unwrap());
    assert!(!fx.home.humidor().join("1.20.0").exists());
    assert!(humidor::entries(&fx.home).unwrap().is_empty());
    assert!(!remove_humidor_mirror(&mut t, &fx.home, &v("1.20.0")).unwrap());
    t.rollback().unwrap();
    assert_eq!(snapshot(&fx.home.root()), before);
}

#[test]
fn a_garbage_humidor_index_is_refused_not_overwritten() {
    let fx = fx();
    fs::create_dir_all(fx.home.humidor()).unwrap();
    fs::write(fx.home.humidor().join("index.json"), "not json").unwrap();
    let mut t = fx.txn("cig");
    let files = fx.staged(&[("bin/cig", b"mine", true)]);
    materialize(&mut t, &fx.home, "cig", &v("1.20.0"), &files, prov()).unwrap();
    let bin = fx.home.toolchain("cig", &v("1.20.0")).join("bin/cig");
    let e = mirror_humidor(&mut t, &fx.home, &v("1.20.0"), &bin).unwrap_err();
    assert_eq!(e.code(), "E1830");
    assert_eq!(
        fs::read_to_string(fx.home.humidor().join("index.json")).unwrap(),
        "not json"
    );
}

// ------------------------------------------------------- state, config, index

#[test]
fn state_save_load_and_rollback() {
    let fx = fx();
    let mut s = State::load(&fx.home).unwrap();
    assert_eq!(s.machine_carton, None);
    s.machine_carton = Some("0.1.0".into());
    let mut t = fx.txn("state");
    s.save_in(&mut t).unwrap();
    t.commit().unwrap();
    assert_eq!(State::load(&fx.home).unwrap(), s);
    let mut s2 = s.clone();
    s2.channel = "beta".into();
    let mut t = fx.txn("state2");
    s2.save_in(&mut t).unwrap();
    t.rollback().unwrap();
    assert_eq!(State::load(&fx.home).unwrap(), s);
    fs::write(fx.home.state_path(), "{\"format\": 9}").unwrap();
    assert_eq!(State::load(&fx.home).unwrap_err().code(), "E1830");
}

#[test]
fn config_set_keeps_comments_and_other_keys() {
    let fx = fx();
    fs::write(
        fx.home.config_path(),
        "# mine\nchannel = beta\nfuture_key = 1\nauto_update = off\n",
    )
    .unwrap();
    let mut c = Config::load(&fx.home).unwrap();
    let mut t = fx.txn("config");
    c.set_in(&mut t, "channel", "nightly").unwrap();
    c.set_in(&mut t, "keep_days", "30").unwrap();
    assert!(c.set_in(&mut t, "channel", "bogus").is_err());
    t.commit().unwrap();
    let text = fs::read_to_string(fx.home.config_path()).unwrap();
    assert_eq!(
        text,
        "# mine\nchannel = nightly\nfuture_key = 1\nauto_update = off\nkeep_days = 30\n"
    );
    let c = Config::load(&fx.home).unwrap();
    assert_eq!(c.get_file("keep_days").as_deref(), Some("30"));
    assert_eq!(c.get_file("auto_update").as_deref(), Some("off"));
}

#[test]
fn state_to_index_through_receipts_and_flip() {
    let fx = fx();
    install_cigo(&fx, "1.17.0", b"d");
    install_cigo(&fx, "1.16.0", b"d0");
    let mut s = State {
        machine_carton: Some("0.1.0".into()),
        ..State::default()
    };
    s.cartons.insert(
        "0.1.0".into(),
        state::CartonRecord {
            installed_at: now_rfc3339(),
            tools: [
                ("cigo".to_string(), "1.17.0".to_string()),
                ("cig".to_string(), "9.9.9".to_string()),
            ]
            .into(),
            reason: "install carton".into(),
            last_used: None,
            partial: false,
        },
    );
    s.sticks.push(state::Stick {
        tool: "cigo".into(),
        version: "1.16.0".into(),
        from_carton: "0.0.9".into(),
        installed_at: now_rfc3339(),
        reason: "pin cigo".into(),
    });
    let idx = ShimIndex::from_state(&fx.home, &s);
    // cig 9.9.9 has no toolchain, so it has no line
    assert_eq!(idx.cartons["0.1.0"].len(), 1);
    // relative to the home, with the identity its receipt recorded at install
    let expect = PathBuf::from("toolchains/cigo/1.17.0/bin/cigo");
    assert_eq!(idx.cartons["0.1.0"]["cigo"], expect);
    let real = fx.home.toolchain("cigo", &v("1.17.0")).join("bin/cigo");
    assert_eq!(
        idx.identities[&expect],
        cigi_shim::Identity::measure(&real).unwrap().render()
    );
    assert!(idx.sticks["cigo"].ends_with("toolchains/cigo/1.16.0/bin/cigo"));

    let mut t = fx.txn("switch");
    s.save_in(&mut t).unwrap();
    idx.flip_in(&mut t).unwrap();
    t.commit().unwrap();
    assert_eq!(ShimIndex::load(&fx.home).unwrap().unwrap(), idx);
    let text = fs::read_to_string(fx.home.shim_index_path()).unwrap();
    assert!(text.starts_with(
        "cigi-index 2\nmachine\t0.1.0\ncarton\t0.1.0\tcigo\ttoolchains/cigo/1.17.0/bin/cigo\t"
    ));
}

#[test]
fn objects_in_use_are_the_keep_set_for_gc() {
    let fx = fx();
    install_cigo(&fx, "1.17.0", b"d");
    let orphan = fx
        .home
        .object_store()
        .put_file(&fx.src("orphan", b"orphan"), false)
        .unwrap();
    let keep: HashSet<ObjectId> = toolchain::referenced_objects(&fx.home)
        .keys()
        .cloned()
        .collect();
    let un = fx.home.object_store().unreferenced(&keep);
    assert_eq!(un.len(), 1);
    assert_eq!(un[0].0, orphan);
    assert_eq!(un[0].1, 6);
}

#[test]
fn removing_a_toolchain_is_journaled() {
    let fx = fx();
    install_cigo(&fx, "1.17.0", b"d");
    let before = snapshot(&fx.home.root());
    let mut t = fx.txn("uninstall");
    toolchain::remove_toolchain(&mut t, &fx.home, "cigo", &v("1.17.0")).unwrap();
    assert!(installed(&fx.home).is_empty());
    t.rollback().unwrap();
    assert_eq!(snapshot(&fx.home.root()), before);
    assert!(verify_toolchain(&fx.home, "cigo", &v("1.17.0")).is_empty());
}

#[test]
fn a_shim_name_that_is_a_path_makes_no_file_anywhere() {
    // names come from manifests and receipts: `../../victim` must not reach outside bin/
    let fx = fx();
    let victim = fx.home.root().join("victim");
    fs::write(&victim, b"PRECIOUS").unwrap();
    let mut t = fx.txn("bad shim");
    let id = t
        .store_file(
            &fx.home.object_store(),
            &fx.src("shim", b"\x7fELF shim"),
            true,
        )
        .unwrap();
    for bad in ["../victim", "../../victim", "a/b", "", "Cig", ".hidden"] {
        let e = sync_shims(&mut t, &fx.home, &[bad], &id).expect_err(bad);
        assert_eq!(e.code(), "E1825", "{bad:?}");
        assert!(remove_shims(&mut t, &fx.home, &[bad]).is_err(), "{bad:?}");
    }
    t.rollback().unwrap();
    assert_eq!(fs::read(&victim).unwrap(), b"PRECIOUS");
    assert!(shims::shims_present(&fx.home, &id, &["../victim"]).is_empty());
}
