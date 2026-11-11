// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

use super::*;
use cigi_shim::pins::{pin_from_carton_file, pin_from_json, pin_from_toml};

const INDEX: &str = "cigi-index 1\nmachine\t0.1.0\n\
carton\t0.1.0\tcig\t/h/cig/1.20.0/cig\ncarton\t0.1.0\tcigo\t/h/cigo/1.17.0/cigo\n\
carton\t0.2.0\tcig\t/h/cig/1.21.0/cig\nstick\tcigcompile\t/h/cc/0.2.0/cigcompile\n\
carton-minor\t0.1\t0.1.0\ncarton-minor\t0.2\t0.2.0\ncarton-major\t0\t0.2.0\n\
legacy-pin-files\t.cig-version\t.cigar-version\n";

fn run(c: Option<&str>, p: &str) -> Pick {
    Pick::Run {
        carton: c.map(String::from),
        path: p.to_string(),
        identity: None,
    }
}

#[test]
fn pin_file_dialects() {
    assert_eq!(
        pin_from_carton_file("# c\n\ncarton 0.1\n"),
        Some("0.1".into())
    );
    assert_eq!(pin_from_carton_file("0.1.2"), Some("0.1.2".into()));
    assert_eq!(
        pin_from_carton_file("carton >=0.1.2"),
        Some(">=0.1.2".into())
    );
    assert_eq!(pin_from_carton_file("# only a comment\n"), None);
    assert_eq!(
        pin_from_json(r#"{"a": {"carton": "9"}, "carton" : "0.1", "b": 1}"#),
        Some("0.1".into())
    );
    assert_eq!(pin_from_json(r#"{"a": {"carton": "9"}}"#), None);
    assert_eq!(
        pin_from_json(r#"{"x": ["carton"], "note": "carton: 1"}"#),
        None
    );
    assert_eq!(
        pin_from_toml("name = \"x\"\ncarton = '0.2'  # c\n[deps]\n"),
        Some("0.2".into())
    );
    assert_eq!(pin_from_toml("[p]\ncarton = \"0.2\"\n"), None);
    assert_eq!(pin_from_toml("cartonx = \"0.2\"\n"), None);
}

#[test]
fn resolution_table() {
    let carton = |s: &str| Pin::Carton(s.to_string());
    assert_eq!(
        pick(INDEX, "cig", None, &carton("0.1.0")),
        run(Some("0.1.0"), "/h/cig/1.20.0/cig")
    );
    assert_eq!(
        pick(INDEX, "cig", None, &carton("v0.2")),
        run(Some("0.2.0"), "/h/cig/1.21.0/cig")
    );
    assert_eq!(
        pick(INDEX, "cig", None, &carton("0")),
        run(Some("0.2.0"), "/h/cig/1.21.0/cig")
    );
    assert_eq!(pick(INDEX, "cig", None, &carton(">=0.1.2")), Pick::Slow);
    assert_eq!(pick(INDEX, "cig", None, &carton("0.3")), Pick::Slow);
    assert_eq!(
        pick(INDEX, "cigo", None, &carton("0.2")),
        Pick::Slow,
        "carton lacks the tool"
    );
    assert_eq!(
        pick(INDEX, "cig", None, &Pin::None),
        run(Some("0.1.0"), "/h/cig/1.20.0/cig")
    );
    assert_eq!(
        pick(INDEX, "cigcompile", None, &Pin::None),
        run(None, "/h/cc/0.2.0/cigcompile")
    );
    assert_eq!(pick(INDEX, "cigar", None, &Pin::None), Pick::Slow);
    assert_eq!(pick(INDEX, "cig", None, &Pin::Slow), Pick::Slow);
    assert_eq!(
        pick(INDEX, "cig", Some("0.2.0"), &carton("0.1")),
        run(Some("0.2.0"), "/h/cig/1.21.0/cig")
    );
    assert_eq!(pick(INDEX, "cig", Some("9.9.9"), &Pin::None), Pick::Slow);
    assert_eq!(pick("cigi-index 2\n", "cig", None, &Pin::None), Pick::Slow);
    assert_eq!(pick("", "cig", None, &Pin::None), Pick::Slow);
}

#[test]
fn legacy_list_is_read_from_the_index() {
    assert_eq!(legacy_names(INDEX), vec![".cig-version", ".cigar-version"]);
    assert!(legacy_names("cigi-index 1\n").is_empty());
}

#[test]
fn pin_grammar_is_cig_compats_digits_and_dots() {
    let carton = |s: &str| Pin::Carton(s.to_string());
    // what `Pin::parse` accepts runs in the shim too, `v` prefixes included
    for ok in ["0.1.0", "v0.1.0", "vv0.2", "0"] {
        assert_ne!(pick(INDEX, "cig", None, &carton(ok)), Pick::Slow, "{ok}");
    }
    // what it rejects goes to the resolver, which says why
    for bad in [
        "0.1.0-beta.1",
        "0.1.0+b",
        "0.1.",
        ".1",
        "0..1",
        "0.1.0.0",
        "0.x",
        "",
        ">=0.1",
    ] {
        assert_eq!(pick(INDEX, "cig", None, &carton(bad)), Pick::Slow, "{bad}");
    }
}

mod installer_mode {
    use crate::installer::*;
    use cigi_shim::{PENDING_CHECK, PREVIOUS_INSTALLER, RESTORED};
    use std::cell::Cell;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    fn home(tag: &str) -> PathBuf {
        let h = std::env::temp_dir().join(format!("cigi-shim-im-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&h);
        std::fs::create_dir_all(h.join("bin")).unwrap();
        std::fs::create_dir_all(h.join("installer").join("previous")).unwrap();
        h
    }

    fn put(h: &Path, parts: &[&str], text: &str) {
        std::fs::write(parts.iter().fold(h.to_path_buf(), |p, s| p.join(s)), text).unwrap();
    }

    fn read(h: &Path, parts: &[&str]) -> Option<String> {
        std::fs::read_to_string(parts.iter().fold(h.to_path_buf(), |p, s| p.join(s))).ok()
    }

    #[test]
    fn name_comes_from_the_stem_and_folds_only_where_asked() {
        assert_eq!(tool_name(Path::new("/h/bin/CIG.EXE"), true), "cig");
        assert_eq!(
            tool_name(Path::new("/h/bin/cigcompile.exe"), true),
            "cigcompile"
        );
        assert_eq!(tool_name(Path::new("/h/bin/cigo"), true), "cigo");
        assert_eq!(tool_name(Path::new("/h/bin/CIG"), false), "CIG");
    }

    #[test]
    fn status_classes() {
        assert!(never_started(Some(0xC000_0135_u32 as i32)));
        assert!(!never_started(Some(0xC000_0005_u32 as i32)));
        assert!(crashed(Some(0xC000_0005_u32 as i32)));
        assert!(!crashed(Some(1)) && !crashed(Some(0)) && !crashed(None));
    }

    #[test]
    fn a_healthy_run_passes_its_code_through_and_touches_nothing() {
        let h = home("ok");
        put(&h, &["installer", "pending-check"], "0.1.1\n");
        put(&h, &["installer", "previous", "ciginstaller.exe"], "old");
        let n = Cell::new(0);
        let code = run_with(
            &h,
            &mut || {
                n.set(n.get() + 1);
                Ok(Some(7))
            },
            Duration::ZERO,
        )
        .unwrap();
        assert_eq!((code, n.get()), (7, 1));
        assert!(read(&h, &PENDING_CHECK).is_some());
    }

    #[test]
    fn a_build_that_cannot_start_is_replaced_by_the_previous_one() {
        let h = home("restore");
        put(&h, &["bin", "ciginstaller.exe"], "new");
        put(&h, &["installer", "pending-check"], "0.1.1\n");
        put(&h, &["installer", "previous", "ciginstaller.exe"], "old");
        let hh = h.clone();
        let code = run_with(
            &h,
            &mut || match read(&hh, &["bin", "ciginstaller.exe"]).as_deref() {
                Some("old") => Ok(Some(0)),
                _ => Ok(Some(0xC000_0135_u32 as i32)),
            },
            Duration::ZERO,
        )
        .unwrap();
        assert_eq!(code, 0);
        assert_eq!(
            read(&h, &["bin", "ciginstaller.exe"]).as_deref(),
            Some("old")
        );
        assert_eq!(read(&h, &RESTORED).as_deref(), Some("0.1.1\n"));
        assert!(read(&h, &PENDING_CHECK).is_none());
        assert_eq!(
            read(&h, &["installer", "previous", "ciginstaller.failed.exe"]).as_deref(),
            Some("new")
        );
    }

    #[test]
    fn a_missing_build_is_restored_too() {
        let h = home("missing");
        put(&h, &["installer", "pending-check"], "0.1.1\n");
        put(&h, &["installer", "previous", "ciginstaller.exe"], "old");
        let hh = h.clone();
        let code = run_with(
            &h,
            &mut || {
                if read(&hh, &["bin", "ciginstaller.exe"]).is_some() {
                    Ok(Some(0))
                } else {
                    Err(std::io::ErrorKind::NotFound.into())
                }
            },
            Duration::ZERO,
        )
        .unwrap();
        assert_eq!(code, 0);
        assert!(read(&h, &PREVIOUS_INSTALLER).is_none());
    }

    #[test]
    fn without_the_marker_nothing_is_restored() {
        let h = home("nomarker");
        put(&h, &["installer", "previous", "ciginstaller.exe"], "old");
        let r = run_with(
            &h,
            &mut || Err(std::io::ErrorKind::PermissionDenied.into()),
            Duration::ZERO,
        );
        assert!(r.is_err());
        assert!(read(&h, &PREVIOUS_INSTALLER).is_some());
    }

    #[test]
    fn a_crash_part_way_restores_but_does_not_run_again() {
        let h = home("crash");
        put(&h, &["bin", "ciginstaller.exe"], "new");
        put(&h, &["installer", "pending-check"], "0.1.1\n");
        put(&h, &["installer", "previous", "ciginstaller.exe"], "old");
        let n = Cell::new(0);
        let code = run_with(
            &h,
            &mut || {
                n.set(n.get() + 1);
                Ok(Some(0xC000_0005_u32 as i32))
            },
            Duration::ZERO,
        )
        .unwrap();
        assert_eq!((code as u32, n.get()), (0xC000_0005, 1));
        assert_eq!(
            read(&h, &["bin", "ciginstaller.exe"]).as_deref(),
            Some("old")
        );
    }
}
