// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! The journal and file rules on Windows: retries, read-only files, identity, containment,
//! reparse points, names, access. Tests marked `windows_` run on the Windows runner only; the
//! rest hold everywhere.

mod common;

use std::fs;
use std::path::Path;
use std::process::Command;

use cigi_home::*;
use common::*;

fn staged_one(fx: &Fx, rel: &str) -> Vec<StagedFile> {
    fx.staged(&[(rel, b"body", false)])
}

// ------------------------------------------------------------------- names

#[test]
fn reserved_and_unportable_manifest_names_are_refused_before_anything_is_made() {
    for bad in [
        "bin/con",
        "NUL.txt",
        "docs/aux.md",
        "com1",
        "lpt9.log",
        "a/b./c",
        "trailing ",
        "ads:stream",
    ] {
        let fx = fx();
        let mut t = fx.txn("install");
        let before = snapshot(&fx.home.root());
        let e = materialize(
            &mut t,
            &fx.home,
            "cigo",
            &v("1.17.0"),
            &staged_one(&fx, bad),
            prov(),
        )
        .unwrap_err();
        assert!(matches!(e, ToolchainError::BadPath(_)), "{bad}: {e}");
        assert!(installed(&fx.home).is_empty(), "{bad}");
        assert_eq!(snapshot(&fx.home.root()), before, "{bad}");
        assert!(!toolchain::safe_rel(bad), "{bad}");
    }
    assert!(toolchain::safe_rel("bin/cigo.exe"));
    assert!(toolchain::safe_rel("docs/console.md"));
}

#[test]
fn a_tool_named_like_a_device_is_not_a_toolchain_name() {
    let fx = fx();
    for tool in ["con", "nul", "com1", "lpt1", "aux", "prn"] {
        assert!(
            remove_toolchain_err(&fx, tool),
            "{tool} must not reach a path"
        );
    }
}

fn remove_toolchain_err(fx: &Fx, tool: &str) -> bool {
    let mut t = fx.txn("remove");
    toolchain::remove_toolchain(&mut t, &fx.home, tool, &v("1.0.0")).is_err()
}

// ----------------------------------------------------- store and the barrier

#[test]
fn a_read_only_store_object_is_flushed_at_the_barrier_and_removed_by_gc() {
    let fx = fx();
    let store = fx.home.object_store();
    let src = fx.src("obj", b"object bytes");
    let mut t = fx.txn("store");
    let id = t.store_file(&store, &src, false).unwrap();
    t.barrier().unwrap();
    t.commit().unwrap();
    store.verify(&id).unwrap();
    // the object is read-only (0444); gc must still be able to remove it
    store.remove(&id).unwrap();
    assert!(!store.has(&id));
}

// ------------------------------------------------------------- the crash sweep

/// Runs only inside a child: an update that adds a toolchain, writes state and flips the index.
#[test]
fn sweep_child() {
    let Ok(root) = std::env::var("CIGI_TEST_SWEEP_HOME") else {
        return;
    };
    let h = Home::at(root.clone().into());
    let l = h.lock().unwrap();
    let stage = Path::new(&root)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("stage");
    let files: Vec<StagedFile> = [
        ("bin/cigo", true),
        ("orchestrate.cig", false),
        ("docs/a.md", false),
    ]
    .iter()
    .map(|(rel, exec)| StagedFile {
        path: (*rel).into(),
        source: stage.join(rel.replace('/', "_")),
        exec: *exec,
    })
    .collect();
    let mut t = Txn::begin(&h, "update", &l).unwrap();
    materialize(&mut t, &h, "cigo", &v("1.17.0"), &files, prov()).unwrap();
    let mut s = State::load(&h).unwrap();
    s.machine_carton = Some("0.2.0".into());
    s.save_in(&mut t).unwrap();
    let idx = ShimIndex {
        machine: Some("0.2.0".into()),
        ..ShimIndex::default()
    };
    idx.flip_in(&mut t).unwrap();
    t.commit().unwrap();
    println!("FINISHED");
}

/// Kill the update at the n-th line of every journal phase until a phase has no n-th line, and
/// after each kill recover and check the home is either as it was or fully updated.
fn sweep() {
    let phases = [
        "begin", "mkdir", "store", "link", "move", "write", "flip", "flipped", "commit",
    ];
    let mut points = 0;
    for phase in phases {
        for n in 1..=60 {
            let fx = fx();
            for (name, body) in [
                ("bin_cigo", "#!/bin/sh\n"),
                ("orchestrate.cig", "orch"),
                ("docs_a.md", "doc"),
            ] {
                fx.src(name, body.as_bytes());
            }
            let mut t = fx.txn("baseline");
            State {
                machine_carton: Some("0.1.0".into()),
                ..State::default()
            }
            .save_in(&mut t)
            .unwrap();
            let old = ShimIndex {
                machine: Some("0.1.0".into()),
                ..ShimIndex::default()
            };
            old.flip_in(&mut t).unwrap();
            t.commit().unwrap();
            let snap = snapshot(&fx.home.root());
            let root = fx.home.root().display().to_string();
            let Fx { tmp, home, lock } = fx;
            drop(lock);
            let out = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "sweep_child", "--nocapture", "--test-threads=1"])
                .env("CIGI_TEST_SWEEP_HOME", root)
                .env("CIGI_FAULT", format!("{phase}:{n}"))
                .output()
                .unwrap();
            let text = String::from_utf8_lossy(&out.stdout).to_string();
            let finished = out.status.success() && text.contains("FINISHED");
            let lock = home.lock().unwrap();
            let rec = recover(&home, &lock).unwrap();
            let updated = State::load(&home).unwrap().machine_carton.as_deref() == Some("0.2.0");
            if updated {
                assert_eq!(installed(&home).len(), 1, "{phase}:{n}");
                assert!(
                    verify_toolchain(&home, "cigo", &v("1.17.0")).is_empty(),
                    "{phase}:{n}"
                );
                assert!(
                    fs::read_to_string(home.shim_index_path())
                        .unwrap()
                        .contains("machine\t0.2.0"),
                    "{phase}:{n}"
                );
            } else {
                assert!(installed(&home).is_empty(), "{phase}:{n}");
                assert_eq!(snapshot(&home.root()), snap, "{phase}:{n}: as before");
            }
            assert!(
                recover(&home, &lock).unwrap().is_empty(),
                "{phase}:{n}: idempotent"
            );
            let _ = rec;
            drop(tmp);
            points += 1;
            if finished {
                break; // this phase has no n-th line
            }
        }
    }
    assert!(points >= phases.len() + 10, "only {points} points tried");
}

#[test]
fn killed_at_every_journal_point_the_home_is_as_it_was_or_fully_updated() {
    sweep();
}

#[cfg(windows)]
#[test]
fn windows_killed_at_every_journal_point_the_home_is_as_it_was_or_fully_updated() {
    sweep();
}

// ---------------------------------------------------------- Windows only

#[cfg(windows)]
mod windows {
    use super::*;
    use std::os::windows::fs::OpenOptionsExt;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    fn hold(path: &Path, share: u32) -> fs::File {
        fs::OpenOptions::new()
            .read(true)
            .share_mode(share)
            .open(path)
            .unwrap()
    }

    fn release_after(handle: fs::File, ms: u64) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(ms));
            drop(handle);
        })
    }

    #[test]
    fn windows_a_held_handle_released_after_a_moment_lets_the_rename_succeed() {
        let t = tempfile::tempdir().unwrap();
        let (a, b) = (t.path().join("a"), t.path().join("b"));
        fs::write(&a, "new").unwrap();
        fs::write(&b, "old").unwrap();
        // share mode 0: nobody may rename, replace or delete while this is open
        let j = release_after(hold(&b, 0), 600);
        let started = Instant::now();
        platform::rename(&a, &b).unwrap();
        assert!(started.elapsed() >= Duration::from_millis(300));
        j.join().unwrap();
        assert_eq!(fs::read_to_string(&b).unwrap(), "new");
    }

    #[test]
    fn windows_a_held_handle_released_after_a_moment_lets_the_delete_succeed() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("f");
        fs::write(&f, "x").unwrap();
        let j = release_after(hold(&f, 0), 600);
        platform::remove_file(&f).unwrap();
        j.join().unwrap();
        assert!(!f.exists());
    }

    #[test]
    fn windows_a_handle_that_is_never_released_fails_after_the_backoff() {
        let t = tempfile::tempdir().unwrap();
        let (a, b) = (t.path().join("a"), t.path().join("b"));
        fs::write(&a, "new").unwrap();
        fs::write(&b, "old").unwrap();
        let _held = hold(&b, 0);
        let started = Instant::now();
        let e = platform::rename(&a, &b).unwrap_err();
        let took = started.elapsed();
        assert!(platform::is_sharing_error(&e), "{e}");
        assert!(
            took >= Duration::from_millis(2000) && took < Duration::from_secs(15),
            "{took:?}"
        );
    }

    #[test]
    fn windows_a_directory_with_an_open_file_says_to_close_running_cig_programs() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("tc");
        fs::create_dir(&d).unwrap();
        fs::write(d.join("tool.exe"), "x").unwrap();
        let _running = hold(&d.join("tool.exe"), 1); // read sharing only: like a loaded program
        let e = platform::rename(&d, &t.path().join("moved")).unwrap_err();
        assert!(e.to_string().contains(platform::CLOSE_RUNNING), "{e}");
        assert!(d.exists());
    }

    #[test]
    fn windows_a_read_only_file_is_replaced_and_deleted() {
        let t = tempfile::tempdir().unwrap();
        let (a, b) = (t.path().join("a"), t.path().join("b"));
        fs::write(&a, "new").unwrap();
        fs::write(&b, "old").unwrap();
        platform::set_mode(&b, 0o444).unwrap();
        assert!(fs::metadata(&b).unwrap().permissions().readonly());
        platform::rename(&a, &b).unwrap();
        assert_eq!(fs::read_to_string(&b).unwrap(), "new");
        platform::set_mode(&b, 0o444).unwrap();
        platform::remove_file(&b).unwrap();
        assert!(!b.exists());
        // a whole tree of read-only files
        let d = t.path().join("tree/sub");
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("f"), "x").unwrap();
        platform::set_mode(&d.join("f"), 0o444).unwrap();
        platform::remove_dir_all(&t.path().join("tree")).unwrap();
        assert!(!t.path().join("tree").exists());
    }

    #[test]
    fn windows_a_toolchain_of_read_only_files_is_removed_and_rolled_back() {
        let fx = fx();
        let mut t = fx.txn("install");
        let files = fx.staged(&[
            ("bin/cigo.exe", b"exe", true),
            ("orchestrate.cig", b"o", false),
        ]);
        materialize(&mut t, &fx.home, "cigo", &v("1.17.0"), &files, prov()).unwrap();
        t.commit().unwrap();
        let mut t = fx.txn("replace");
        // a changed file replaces a read-only one through the journal, then rolls back
        let obj = fx.src("changed", b"changed");
        let tc = fx
            .home
            .toolchain("cigo", &v("1.17.0"))
            .join("orchestrate.cig");
        t.link(&obj, &tc).unwrap();
        assert_eq!(fs::read(&tc).unwrap(), b"changed");
        t.rollback().unwrap();
        assert_eq!(fs::read(&tc).unwrap(), b"o");
        toolchain::remove_toolchain_now(&fx.home, "cigo", &v("1.17.0")).unwrap();
        assert!(installed(&fx.home).is_empty());
    }

    #[test]
    fn windows_file_identity_follows_hard_links_not_names() {
        let t = tempfile::tempdir().unwrap();
        let (a, b, c) = (t.path().join("a"), t.path().join("b"), t.path().join("c"));
        fs::write(&a, "x").unwrap();
        fs::write(&c, "x").unwrap();
        fs::hard_link(&a, &b).unwrap();
        assert!(
            platform::ino(&a).is_some(),
            "identity must exist on Windows"
        );
        assert!(platform::same_file(&a, &b));
        assert!(!platform::same_file(&a, &c));
        assert_eq!(platform::nlink(&a), Some(2));
        // a different spelling of one path is one file, and `canonicalize` is not consulted
        let shouty = PathBuf::from(a.to_string_lossy().to_uppercase());
        assert!(platform::same_file(&a, &shouty));
        // a directory has an identity too
        assert!(platform::file_id(t.path()).is_some());
    }

    #[test]
    fn windows_containment_ignores_case_and_the_verbatim_prefix() {
        let fx = fx();
        let root = fx.home.root();
        let upper = PathBuf::from(root.to_string_lossy().to_uppercase());
        assert!(platform::starts_with(&upper.join("bin"), &root));
        let verbatim = PathBuf::from(format!(r"\\?\{}", root.display()));
        assert!(platform::starts_with(&verbatim.join("bin"), &root));
        // the journal's own containment check accepts the other spelling
        let victim = root.join("bin").join("x");
        fs::write(&victim, "x").unwrap();
        let mut t = fx.txn("delete");
        t.delete(&upper.join("bin").join("x")).unwrap();
        t.commit().unwrap();
        assert!(!victim.exists());
    }

    fn junction(link: &Path, target: &Path) {
        let st = Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .unwrap();
        assert!(
            st.status.success(),
            "{}",
            String::from_utf8_lossy(&st.stdout)
        );
    }

    #[test]
    fn windows_a_junction_is_refused_and_never_followed() {
        let fx = fx();
        let outside = fx.tmp.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("precious"), "keep").unwrap();
        let link = fx.home.root().join("bin").join("jct");
        junction(&link, &outside);
        assert!(platform::is_link(&link));
        assert!(!platform::is_link(&outside));
        let mut t = fx.txn("delete");
        // through the junction: refused with the path it will not follow (E1869)
        let e = t.delete(&link.join("precious")).unwrap_err();
        assert_eq!(e.code(), "E1869", "{e}");
        // the junction itself goes, as a link; what it pointed at stays
        t.delete(&link).unwrap();
        assert!(!link.exists());
        assert_eq!(fs::read(outside.join("precious")).unwrap(), b"keep");
        // a toolchain directory that is a junction is not removed through
        let tc = fx.home.toolchains().join("cigo");
        junction(&tc, &outside);
        assert!(toolchain::remove_toolchain(&mut t, &fx.home, "cigo", &v("1.0.0")).is_err());
        assert_eq!(fs::read(outside.join("precious")).unwrap(), b"keep");
        // a writer will not write through a link either
        let writer = fx.home.root().join("bin").join("jct2");
        junction(&writer, &outside);
        assert!(t.write(&writer, b"x", None).is_err());
    }

    #[test]
    fn windows_a_300_character_home_installs_verifies_and_removes() {
        let tmp = tempfile::tempdir().unwrap();
        let mut root = tmp.path().to_path_buf();
        for i in 0..8 {
            root.push(format!("segment-{i}-{}", "d".repeat(30)));
        }
        root.push(".cigscript");
        assert!(root.to_string_lossy().len() > 300);
        let home = Home::at(root);
        assert!(home.long_path_warning().is_some());
        home.ensure_layout().unwrap();
        let lock = home.lock().unwrap();
        let stage = tmp.path().join("stage");
        fs::create_dir_all(&stage).unwrap();
        fs::write(stage.join("a"), "alpha").unwrap();
        let files = vec![StagedFile {
            path: "docs/deep/deeper/file.md".into(),
            source: stage.join("a"),
            exec: false,
        }];
        let mut t = Txn::begin(&home, "install", &lock).unwrap();
        materialize(&mut t, &home, "cigo", &v("1.17.0"), &files, prov()).unwrap();
        t.commit().unwrap();
        assert!(verify_toolchain(&home, "cigo", &v("1.17.0")).is_empty());
        let mut t = Txn::begin(&home, "remove", &lock).unwrap();
        toolchain::remove_toolchain(&mut t, &home, "cigo", &v("1.17.0")).unwrap();
        t.commit().unwrap();
        assert!(installed(&home).is_empty());
    }

    #[test]
    fn windows_the_write_through_rename_replaces_a_file_and_moves_a_directory() {
        let t = tempfile::tempdir().unwrap();
        let (a, b) = (t.path().join("a"), t.path().join("b"));
        fs::write(&a, "1").unwrap();
        fs::write(&b, "2").unwrap();
        platform::rename(&a, &b).unwrap();
        assert_eq!(fs::read_to_string(&b).unwrap(), "1");
        let d = t.path().join("d");
        fs::create_dir(&d).unwrap();
        fs::write(d.join("f"), "x").unwrap();
        platform::rename(&d, &t.path().join("d2")).unwrap();
        assert!(t.path().join("d2").join("f").exists());
    }

    #[test]
    fn windows_a_directory_under_a_foreign_owner_fails_the_ownership_check() {
        let t = tempfile::tempdir().unwrap();
        assert!(platform::dir_ok(t.path()).is_ok());
        let e = platform::dir_ok(Path::new(r"C:\Windows")).unwrap_err();
        assert!(e.contains("another user"), "{e}");
    }

    #[test]
    fn windows_a_protected_directory_is_the_current_users_alone() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("private");
        fs::create_dir(&d).unwrap();
        cigi_sys::fs::protect_dir_current_user(&d).unwrap();
        assert!(platform::dir_ok(&d).is_ok());
        fs::write(d.join("inside"), "x").unwrap(); // the user is in
        let out = Command::new("icacls").arg(&d).output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        assert!(text.contains("(F)"), "{text}");
        assert!(
            !text.contains("BUILTIN\\Users") && !text.contains("Everyone"),
            "{text}"
        );
        // what is made inside inherits it
        let out = Command::new("icacls")
            .arg(d.join("inside"))
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        assert!(!text.contains("BUILTIN\\Users"), "{text}");
    }
}
