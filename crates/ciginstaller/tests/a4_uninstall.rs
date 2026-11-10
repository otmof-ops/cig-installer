// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! Phase 4 acceptance (SPEC 14.3): `uninstall <tool>`, `uninstall --all`, `remove <carton>`.
//! `--all` leaves only the user's data, at every kill point.

// Unix only: uninstall tests run shell-script fake products and set Unix modes.
// The Windows runtime drafts add their own equivalents.
#![cfg(unix)]

mod common;

use cigi_testkit::{Fixture, Snapshot};
use common::{cigi, fixture, ok, text};
use serde_json::Value;

fn carton_fixture() -> Fixture {
    let mut fx = fixture();
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.16.0")]);
    fx.carton("0.2.0", &[("cig", "1.21.0"), ("cigo", "1.17.0")]);
    fx.publish("stable");
    fx
}

/// The user's own files, made before the install: cig's run records, the tin, cig's config, and
/// a profile with a line of the user's in it.
fn user_data(fx: &Fixture, profile: bool) {
    let root = fx.cigscript();
    std::fs::create_dir_all(root.join("runs/r1")).unwrap();
    // the installer refuses a home others can write to (4.1); a test umask may allow it
    std::fs::set_permissions(&root, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
    std::fs::write(root.join("runs/r1/run.json"), "{\"version\":\"1.20.0\"}\n").unwrap();
    std::fs::create_dir_all(root.join("tin")).unwrap();
    std::fs::write(root.join("tin/undo-1"), "saved\n").unwrap();
    std::fs::write(root.join("config"), "mode = tidy\n").unwrap();
    if profile {
        std::fs::write(fx.user_home.join(".profile"), "export FOO=1\n").unwrap();
    }
}

fn json(o: &std::process::Output) -> Value {
    let line = String::from_utf8_lossy(&o.stdout)
        .lines()
        .last()
        .unwrap_or("")
        .to_string();
    serde_json::from_str::<Value>(&line).unwrap_or_else(|e| panic!("{e}: {}", text(o)))
}

/// Home listing without the parts that change with every command (the lock's pid).
fn listing(fx: &Fixture) -> Snapshot {
    Snapshot::take(&fx.user_home).without(&[
        ".cigscript/installer/lock",
        ".cigscript/installer.lock",
        ".cigscript/installer/lock.pid",
    ])
}

fn asides(fx: &Fixture) -> Vec<String> {
    std::fs::read_dir(&fx.user_home)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(".cigscript-uninstall-"))
        .collect()
}

#[test]
fn a4_uninstall_all_leaves_only_the_users_data() {
    for profile in [true, false] {
        let fx = carton_fixture();
        user_data(&fx, profile);
        let before_install = Snapshot::take(&fx.user_home);
        let profile_bytes = std::fs::read(fx.user_home.join(".profile")).ok();
        ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
        ok(&cigi(&fx, &["install", "carton", "--carton", "0.2.0"]));
        assert!(fx.cigscript().join("bin/cigo").exists());
        let t = std::fs::read_to_string(fx.user_home.join(".profile")).unwrap();
        assert_eq!(t.matches("# >>> cigscript >>>").count(), 1);

        let o = cigi(&fx, &["--json", "uninstall", "--all", "--yes-irreversible"]);
        ok(&o);
        let d = json(&o)["data"].clone();
        assert_eq!(d["changed"], true);
        // it prints every path it removed
        let removed: Vec<String> = d["removed"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        let has = |frag: &str| removed.iter().any(|p| p.contains(frag));
        assert!(has("bin/cigo"), "{removed:#?}");
        assert!(has("bin/ciginstaller"), "{removed:#?}");
        assert!(has("toolchains/cigo/1.16.0"), "{removed:#?}");
        assert!(has("toolchains/cig/1.21.0"), "{removed:#?}");
        assert!(has("/store"), "{removed:#?}");
        assert!(has("humidor/1.20.0"), "{removed:#?}");
        assert!(has("the PATH block"), "{removed:#?}");
        assert!(has("/installer"), "{removed:#?}");
        // the guard's pieces do not exist in this build, and it says so
        let plain = cigi(
            &fx,
            &["--plain", "uninstall", "--all", "--yes-irreversible"],
        );
        assert!(
            text(&plain).contains("nothing to uninstall"),
            "{}",
            text(&plain)
        );

        // exactly the home as it was before the install: the user's files, nothing else
        Snapshot::take(&fx.user_home).assert_same(&before_install);
        assert_eq!(
            std::fs::read(fx.user_home.join(".profile")).ok(),
            profile_bytes
        );
        assert!(asides(&fx).is_empty());
        // and it can all be put back
        ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
        assert!(fx.cigscript().join("bin/cigo").exists());
    }
}

#[test]
fn a4_uninstall_purge_is_refused_and_a_question_needs_a_yes() {
    let fx = carton_fixture();
    user_data(&fx, true);
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    let before = listing(&fx);
    for args in [
        &["uninstall", "--all", "--purge"][..],
        &["uninstall", "--purge"][..],
        &["uninstall", "cigo", "--purge"][..],
    ] {
        let o = cigi(&fx, args);
        assert_eq!(o.status.code(), Some(2), "{args:?}: {}", text(&o));
        assert!(text(&o).contains("E1861"), "{}", text(&o));
        listing(&fx).assert_same(&before);
    }
    // without --yes and without a terminal the safe default is no
    let o = fx
        .cigi()
        .env("CIGI_ALLOW_SINGLE_SOURCE", "true")
        .args(["uninstall", "--all"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(3), "{}", text(&o));
    assert!(text(&o).contains("E1871"), "{}", text(&o));
    listing(&fx).assert_same(&before);
    // `--yes` alone does not answer an irreversible question
    let o = cigi(&fx, &["uninstall", "--all"]);
    assert_eq!(o.status.code(), Some(3), "{}", text(&o));
    assert!(text(&o).contains("E1871"), "{}", text(&o));
    assert!(text(&o).contains("--yes-irreversible"), "{}", text(&o));
    listing(&fx).assert_same(&before);
    // naming nothing is a usage error
    let o = cigi(&fx, &["uninstall"]);
    assert_eq!(o.status.code(), Some(3));
    let o = cigi(&fx, &["uninstall", "cigo", "--all"]);
    assert_eq!(o.status.code(), Some(3));
    listing(&fx).assert_same(&before);
}

#[test]
fn a4_uninstall_all_dry_run_and_nothing_installed() {
    let fx = carton_fixture();
    let o = cigi(&fx, &["uninstall", "--all", "--yes-irreversible"]);
    ok(&o);
    assert!(text(&o).contains("nothing to uninstall"));
    assert!(!fx.cigscript().exists(), "it made a home to remove it");
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    let before = listing(&fx);
    let o = cigi(&fx, &["--json", "uninstall", "--all", "--dry-run"]);
    ok(&o);
    assert_eq!(json(&o)["data"]["dry_run"], true);
    listing(&fx).assert_same(&before);
}

#[test]
fn a4_uninstall_all_keep_journal_and_snapshot() {
    let fx = carton_fixture();
    user_data(&fx, false);
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    let o = cigi(
        &fx,
        &[
            "--json",
            "uninstall",
            "--all",
            "--keep-journal",
            "--yes-irreversible",
        ],
    );
    ok(&o);
    let kept = asides(&fx);
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert_eq!(
        json(&o)["data"]["journal"].as_str().unwrap(),
        fx.user_home.join(&kept[0]).display().to_string()
    );
    let dir = fx.user_home.join(&kept[0]);
    let snap: Value =
        serde_json::from_slice(&std::fs::read(dir.join("uninstall-snapshot.json")).unwrap())
            .unwrap();
    let entries = snap["entries"].as_array().unwrap();
    assert!(entries
        .iter()
        .any(|e| e["path"] == "bin/cigo" && e["sha256"].as_str().is_some_and(|s| s.len() == 64)));
    assert!(entries.iter().any(|e| e["path"]
        .as_str()
        .is_some_and(|p| p.starts_with("toolchains/cig/1.20.0/"))
        && e["kind"] == "file"));
    // the journal ended with its commit line
    let id = snap["txn"].as_str().unwrap();
    let j = std::fs::read_to_string(dir.join("txn").join(id).join("journal.jsonl")).unwrap();
    assert!(j.lines().last().unwrap().contains("\"commit\""), "{j}");
    // a kept journal is not a finding
    let o = cigi(&fx, &["--json", "doctor"]);
    ok(&o);
}

/// Killed at every point, `uninstall --all` leaves the whole home or only the user's data. Before
/// the move of `installer/` the next command undoes it; after it there is nothing to undo.
#[test]
fn a4_uninstall_all_kill_sweep() {
    let (mut crashes, mut pre_seen, mut post_seen) = (0, 0, 0);
    let points: Vec<String> = ["write", "delete", "block", "commit"]
        .iter()
        .flat_map(|p| (1..80).map(move |n| format!("{p}:{n}")))
        .chain([
            "uninstall-before-aside:1".to_string(),
            "uninstall-aside:1".to_string(),
        ])
        .collect();
    let mut skip_phase: Option<String> = None;
    for fault in points {
        let phase = fault.split(':').next().unwrap().to_string();
        if skip_phase.as_deref() == Some(phase.as_str()) {
            continue;
        }
        let fx = carton_fixture();
        user_data(&fx, true);
        let user_only = Snapshot::take(&fx.user_home);
        ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
        let whole = listing(&fx);
        let o = fx
            .cigi()
            .env("CIGI_ALLOW_SINGLE_SOURCE", "true")
            .env("CIGI_FAULT", &fault)
            .args(["--yes-irreversible", "uninstall", "--all"])
            .output()
            .unwrap();
        if o.status.code().is_some() {
            skip_phase = Some(phase); // this phase has fewer lines than that
            continue;
        }
        crashes += 1;
        let ignore: Vec<String> = asides(&fx);
        let ignore: Vec<&str> = ignore.iter().map(String::as_str).collect();
        if fx.cigscript().join("installer").exists() {
            // before the commit point: the next command that takes the lock undoes it
            let o = cigi(&fx, &["gc", "--dry-run"]);
            ok(&o);
            let now = listing(&fx).without(&[".cigscript/installer/txn"]);
            let want = whole.without(&[".cigscript/installer/txn"]);
            assert!(
                now.diff(&want).is_empty(),
                "{fault}: not whole after recovery: {:?}\n{}",
                now.diff(&want),
                text(&o)
            );
            assert!(asides(&fx).is_empty(), "{fault}");
            pre_seen += 1;
        } else {
            // after it: installer/ is gone and so is everything of the family
            let now = Snapshot::take(&fx.user_home).without(&ignore);
            assert!(
                now.diff(&user_only).is_empty(),
                "{fault}: not only the user's data: {:?}",
                now.diff(&user_only)
            );
            // a journal that did not get to delete itself is a finding, and `uninstall --all` clears it
            if !ignore.is_empty() {
                let o = cigi(&fx, &["--json", "doctor"]);
                let d = json(&o)["data"].clone();
                let flagged = d["findings"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|f| f["code"] == "E1865" && f["severity"] == "action");
                assert!(flagged, "{fault}: {d:#}");
                let fix = d["findings"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|f| f["code"] == "E1865")
                    .map(|f| f["fix"].clone());
                assert_eq!(fix, Some(Value::from("ciginstaller gc")), "{fault}");
                // `gc` clears it (and lays the home out again, as any command does): the user's
                // files are all still there, unchanged
                ok(&cigi(&fx, &["gc"]));
                assert!(asides(&fx).is_empty(), "{fault}");
                let after = Snapshot::take(&fx.user_home);
                assert!(
                    user_only
                        .diff(&after)
                        .iter()
                        .all(|d| d.starts_with("added:")),
                    "{fault}: {:?}",
                    user_only.diff(&after)
                );
            }
            post_seen += 1;
        }
    }
    assert!(crashes >= 6, "only {crashes} fault points reached");
    assert!(
        pre_seen > 0 && post_seen > 0,
        "pre {pre_seen}, post {post_seen}"
    );
}

#[test]
fn a4_uninstall_one_tool() {
    let fx = carton_fixture();
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    let o = cigi(&fx, &["--json", "uninstall", "cigo"]);
    ok(&o);
    let root = fx.cigscript();
    assert!(!root.join("toolchains/cigo").exists());
    assert!(!root.join("bin/cigo").exists());
    assert!(root.join("bin/cig").exists());
    let idx = std::fs::read_to_string(root.join("installer/shim-index")).unwrap();
    // no line routes to cigo any more (`.cigo-version` in the legacy pin file list stays)
    assert!(
        !idx.contains("\tcigo\t") && !idx.contains("toolchains/cigo"),
        "{idx}"
    );
    let st: Value =
        serde_json::from_slice(&std::fs::read(root.join("installer/state.json")).unwrap()).unwrap();
    assert_eq!(st["cartons"]["0.1.0"]["partial"], true);
    assert!(st["cartons"]["0.1.0"]["tools"].get("cigo").is_none());
    assert!(
        st["extra"].is_null() && st.get("pending_deletes").is_none(),
        "pending deletes were not cleared: {st}"
    );
    // cigo's own objects are freed; cig still runs
    let o = fx.run_shim("cig", &fx.project, &["--version"]);
    ok(&o);
    // what is left is consistent
    let o = cigi(&fx, &["--json", "doctor"]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    // twice, or something that is not there, is E1863
    let o = cigi(&fx, &["uninstall", "cigo"]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(text(&o).contains("E1863"));
    let o = cigi(&fx, &["uninstall", "ciginstaller"]);
    assert!(text(&o).contains("E1863"));
    // cig takes its humidor mirror with it
    ok(&cigi(&fx, &["uninstall", "cig"]));
    assert!(!root.join("humidor/1.20.0").exists());
    assert!(!std::fs::read_to_string(root.join("humidor/index.json"))
        .unwrap_or_default()
        .contains("1.20.0"));
}

#[test]
fn a4_remove_carton() {
    let fx = carton_fixture();
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.2.0"]));
    let root = fx.cigscript();
    // the machine's carton stays
    let o = cigi(&fx, &["remove", "0.1.0"]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(text(&o).contains("E1863"));
    let o = cigi(&fx, &["remove", "9.9.9"]);
    assert!(text(&o).contains("E1863"));
    ok(&cigi(&fx, &["remove", "0.2.0"]));
    assert!(!root.join("toolchains/cigo/1.17.0").exists());
    assert!(!root.join("toolchains/cig/1.21.0").exists());
    // 0.1.0 is whole, its files shared with 0.2.0 were not taken
    assert!(root.join("toolchains/cigo/1.16.0/LICENSE").exists());
    let o = cigi(&fx, &["--json", "doctor"]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    let st: Value =
        serde_json::from_slice(&std::fs::read(root.join("installer/state.json")).unwrap()).unwrap();
    assert!(st["cartons"].get("0.2.0").is_none());
}
