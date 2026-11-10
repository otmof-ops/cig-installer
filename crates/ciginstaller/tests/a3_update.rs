// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! Phase 3 acceptance, the update lifecycle (SPEC 14.3): update, the trial, the switch, rollback.

mod common;

use std::path::Path;
use std::process::{Command, Output, Stdio};

use cigi_testkit::{Fixture, PublishOpts, Snapshot};
use common::{cigi, fixture, ok, out, text, type_at_prompts};

fn scripts(fx: &Fixture, n: usize) {
    for i in 0..n {
        std::fs::write(
            fx.project.join(format!("s{i}.cig")),
            format!("exhale \"script {i}\"\n"),
        )
        .unwrap();
    }
}

/// 0.1.0 has cig 1.20.0, 0.1.1 has cig 1.21.0; cigo 1.17.0 is in both. Both cigs plan alike
/// unless `new_plan` says the new one plans differently.
fn two(fx: &mut Fixture, new_plan: &str) {
    fx.product("cig", "1.20.0").dry_run_output("plan: 1 op");
    fx.product("cig", "1.21.0").dry_run_output(new_plan);
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.17.0")]);
    fx.carton("0.1.1", &[("cig", "1.21.0"), ("cigo", "1.17.0")]);
    fx.publish("stable");
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_default()
}

/// The part of the home a rollback must give back: what is installed and what the shims read.
fn installed_files(fx: &Fixture) -> Snapshot {
    let all = Snapshot::take(&fx.cigscript());
    Snapshot {
        entries: all
            .entries
            .into_iter()
            .filter(|(p, _)| {
                ["toolchains", "bin", "humidor"]
                    .iter()
                    .any(|x| p == x || p.starts_with(&format!("{x}/")))
            })
            .collect(),
    }
}

fn index(fx: &Fixture) -> String {
    read(&fx.cigscript().join("installer/shim-index"))
}

/// The state the user can see, without what only the bookkeeping of a switch changes.
fn state(fx: &Fixture) -> serde_json::Value {
    let mut v: serde_json::Value =
        serde_json::from_str(&read(&fx.cigscript().join("installer/state.json"))).unwrap();
    let o = v.as_object_mut().unwrap();
    for k in ["history", "manifest", "manifest_digests", "update_undo"] {
        o.remove(k);
    }
    v
}

fn history(fx: &Fixture) -> Vec<String> {
    let v: serde_json::Value =
        serde_json::from_str(&read(&fx.cigscript().join("installer/state.json"))).unwrap();
    v["history"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["what"].as_str().unwrap().to_string())
        .collect()
}

fn which_cig(fx: &Fixture) -> String {
    let o = fx.cigi().args(["which", "cig"]).output().unwrap();
    ok(&o);
    text(&o)
}

fn raw(fx: &Fixture, args: &[&str]) -> Output {
    fx.cigi()
        .env("CIGI_ALLOW_SINGLE_SOURCE", "true")
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn a3_update_trial_switch_rollback() {
    let mut fx = fixture();
    two(&mut fx, "plan: 1 op");
    scripts(&fx, 3);
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    // `which` records when a carton was last used: ask before the snapshot, not after
    assert!(which_cig(&fx).contains("cig 1.20.0"));
    let (files0, index0, state0) = (installed_files(&fx), index(&fx), state(&fx));

    let o = cigi(&fx, &["update"]);
    ok(&o);
    let t = text(&o);
    assert!(t.contains("updated carton 0.1.0 to 0.1.1"), "{t}");
    assert!(t.contains("cig 1.20.0 -> 1.21.0"), "{t}");
    assert!(
        t.contains("trial: 3 script(s) compared, 3 the same, 0 different"),
        "{t}"
    );
    assert!(t.contains("ciginstaller rollback"), "{t}");
    assert!(which_cig(&fx).contains("cig 1.21.0"), "{}", which_cig(&fx));
    let after: serde_json::Value = state(&fx);
    assert_eq!(after["machine_carton"], "0.1.1");
    assert_eq!(after["previous"]["machine_carton"], "0.1.0");
    assert_ne!(index(&fx), index0);
    // the shim runs the new cig
    let o = fx.run_shim("cig", &fx.project, &["--version"]);
    assert_eq!(out(&o), "cig 1.21.0");
    // the old toolchains were not touched
    let still = installed_files(&fx);
    for (p, e) in &files0.entries {
        if p.starts_with("toolchains/") {
            assert_eq!(still.entries.get(p), Some(e), "{p} changed by the update");
        }
    }

    let o = cigi(&fx, &["rollback"]);
    ok(&o);
    assert!(
        text(&o).contains("rolled back carton 0.1.1 to 0.1.0"),
        "{}",
        text(&o)
    );
    files0.assert_same(&installed_files(&fx));
    assert_eq!(
        index(&fx),
        index0,
        "the shim-index is byte for byte what it was"
    );
    assert_eq!(
        state(&fx),
        state0,
        "the state differs only in history and the manifest record"
    );
    assert_eq!(history(&fx), ["install", "update", "rollback"]);
    assert!(which_cig(&fx).contains("cig 1.20.0"));
    // nothing left to roll back to
    let o = cigi(&fx, &["rollback"]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(text(&o).contains("E1852"), "{}", text(&o));
}

#[test]
fn a3_update_dry_run_and_up_to_date_change_nothing() {
    let mut fx = fixture();
    two(&mut fx, "plan: 1 op");
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    let quiet = ["installer/cache", "installer/txn", "installer/lock"];
    let before = Snapshot::take(&fx.cigscript()).without(&quiet);
    let o = cigi(&fx, &["update", "--dry-run"]);
    ok(&o);
    assert!(
        text(&o).contains("would update carton 0.1.0 to 0.1.1"),
        "{}",
        text(&o)
    );
    assert!(
        text(&o).contains("cig 1.21.0: fetch and verify"),
        "{}",
        text(&o)
    );
    assert!(
        text(&o).contains("cigo 1.17.0: already here"),
        "{}",
        text(&o)
    );
    Snapshot::take(&fx.cigscript())
        .without(&quiet)
        .assert_same(&before);
    ok(&cigi(&fx, &["update"]));
    let o = cigi(&fx, &["update"]);
    ok(&o);
    assert!(text(&o).contains("is the newest"), "{}", text(&o));
}

/// A TTY for the child through util-linux `script`; its input is `keys`.
fn on_tty(fx: &Fixture, args: &str, keys: &[&str], env: &[(&str, &str)]) -> (i32, String) {
    let c = fx.cigi();
    let exe = c.get_program().to_string_lossy().to_string();
    let q = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
    let line = format!("{} {args}", q(&exe));
    let mut s = Command::new("script");
    s.env_clear();
    for (k, v) in c.get_envs() {
        if let Some(v) = v {
            s.env(k, v);
        }
    }
    s.envs(env.iter().copied());
    s.env("CIGI_ALLOW_SINGLE_SOURCE", "true")
        .args(["-qec", &line, "/dev/null"])
        .current_dir(&fx.project)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    type_at_prompts(s, keys)
}

#[test]
fn a3_trial_difference() {
    let mut fx = fixture();
    two(&mut fx, "plan: 2 ops");
    scripts(&fx, 2);
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    let files0 = installed_files(&fx);

    // no terminal: refused, exit 2, E1851, nothing switched
    let o = raw(&fx, &["update"]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(text(&o).contains("E1851"), "{}", text(&o));
    assert!(
        text(&o).contains("2 of 2 script(s) planning differently"),
        "{}",
        text(&o)
    );
    // --yes accepts identical results only
    let o = cigi(&fx, &["update"]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(text(&o).contains("E1851"), "{}", text(&o));
    files0.assert_same(&installed_files(&fx));
    assert!(which_cig(&fx).contains("cig 1.20.0"));

    // --trial-only shows the differences and keeps nothing
    let o = cigi(&fx, &["update", "--trial-only"]);
    ok(&o);
    assert!(text(&o).contains("-plan: 1 op"), "{}", text(&o));
    assert!(text(&o).contains("+plan: 2 ops"), "{}", text(&o));
    files0.assert_same(&installed_files(&fx));

    // on a terminal: the prompt, and keep leaves 0.1.0
    let (code, t) = on_tty(&fx, "update", &["k"], &[]);
    assert!(
        t.contains("ccept") && t.contains("eview") && t.contains("eep"),
        "{t}"
    );
    assert_eq!(code, 2, "{t}");
    assert!(t.contains("E1851"), "{t}");
    files0.assert_same(&installed_files(&fx));
    assert!(which_cig(&fx).contains("cig 1.20.0"));
    // an empty answer is the default, which is keep
    let (code, t) = on_tty(&fx, "update", &["\n"], &[]);
    assert_eq!(code, 2, "{t}");
    // review shows the diff, then accept switches
    let (code, t) = on_tty(&fx, "update", &["r", "a"], &[]);
    assert_eq!(code, 0, "{t}");
    assert!(t.contains("+plan: 2 ops"), "{t}");
    assert!(which_cig(&fx).contains("cig 1.21.0"));
}

#[test]
fn a3_yes_differences_accepts() {
    let mut fx = fixture();
    two(&mut fx, "plan: 2 ops");
    scripts(&fx, 1);
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    ok(&cigi(&fx, &["update", "--yes-differences"]));
    assert!(which_cig(&fx).contains("cig 1.21.0"));
}

#[test]
fn a3_no_project_or_no_trial_skips_the_trial() {
    let mut fx = fixture();
    two(&mut fx, "plan: 2 ops");
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    // no .cig file or pin anywhere: no project, so nothing to try, and the update goes ahead
    let o = cigi(&fx, &["update"]);
    ok(&o);
    assert!(
        text(&o).contains("no project here, trial skipped"),
        "{}",
        text(&o)
    );
    ok(&cigi(&fx, &["rollback"]));
    scripts(&fx, 1);
    let o = cigi(&fx, &["update", "--no-trial"]);
    ok(&o);
    assert!(text(&o).contains("trial skipped"), "{}", text(&o));
}

#[test]
fn a3_update_only_changed() {
    let mut fx = fixture();
    two(&mut fx, "plan: 1 op");
    let server = fx.serve();
    let run = |args: &[&str]| {
        let o = fx
            .cigi_with_mirror(&server.url())
            .env("CIGI_ALLOW_SINGLE_SOURCE", "true")
            .arg("--yes")
            .args(args)
            .output()
            .unwrap();
        ok(&o);
    };
    run(&["install", "carton", "--carton", "0.1.0"]);
    server.clear();
    run(&["update"]);
    let paths = server.paths();
    assert!(
        paths.iter().any(|p| p.contains("cig-v1.21.0")),
        "the changed tool is fetched: {paths:?}"
    );
    assert!(
        !paths.iter().any(|p| p.contains("cig-orchestrator")),
        "cigo did not change, so its asset is not requested: {paths:?}"
    );
    assert!(
        !paths.iter().any(|p| p.contains("cig-v1.20.0")),
        "{paths:?}"
    );
}

#[test]
fn a3_yank_and_downgrade() {
    let mut fx = fixture();
    fx.carton("0.1.0", &[("cig", "1.20.0")]);
    fx.carton("0.2.0", &[("cig", "1.21.0")]);
    fx.carton("0.3.0", &[("cig", "1.22.0")]);
    fx.yank_carton("0.3.0", "broken in the fixture");
    fx.publish("stable");
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.2.0"]));
    let files0 = installed_files(&fx);

    // the yanked target is refused, and a plain update does not choose it
    let o = cigi(&fx, &["update", "--to", "0.3.0"]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(text(&o).contains("E1803"), "{}", text(&o));
    let o = cigi(&fx, &["update"]);
    ok(&o);
    assert!(text(&o).contains("newest"), "{}", text(&o));

    // a lower target needs --allow-downgrade
    let o = cigi(&fx, &["update", "--to", "0.1.0"]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(text(&o).contains("E1850"), "{}", text(&o));
    assert!(text(&o).contains("--allow-downgrade"), "{}", text(&o));
    files0.assert_same(&installed_files(&fx));
    let o = cigi(&fx, &["update", "--to", "0.1.0", "--allow-downgrade"]);
    ok(&o);
    assert!(which_cig(&fx).contains("cig 1.20.0"));

    // a manifest whose sequence went backwards is refused
    fx.publish_with(
        "stable",
        &PublishOpts {
            sequence: Some(1),
            ..PublishOpts::default()
        },
    );
    let o = cigi(&fx, &["update"]);
    assert_ne!(o.status.code(), Some(0), "{}", text(&o));
    assert!(text(&o).contains("E1824"), "{}", text(&o));
    // an expired manifest is refused
    fx.publish_with(
        "stable",
        &PublishOpts {
            sequence: Some(99),
            expires_in_days: Some(-1),
            ..PublishOpts::default()
        },
    );
    let o = cigi(&fx, &["update"]);
    assert_ne!(o.status.code(), Some(0), "{}", text(&o));
    assert!(text(&o).contains("E1823"), "{}", text(&o));
    assert!(which_cig(&fx).contains("cig 1.20.0"));
}

#[test]
fn a3_check_writes_nothing_and_exit_code_is_10() {
    let mut fx = fixture();
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.17.0")]);
    fx.publish("stable");
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    // the cached manifest is under a day old: --check uses it and sees nothing newer
    let o = cigi(&fx, &["update", "--check", "--exit-code"]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    fx.carton("0.1.2", &[("cig", "1.22.0"), ("cigo", "1.17.0")]);
    fx.publish("stable");
    // still the cache: a newer carton is out there, and --check without --refresh does not ask
    let o = cigi(&fx, &["update", "--check", "--exit-code"]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    let before = Snapshot::take(&fx.cigscript());
    let o = cigi(&fx, &["update", "--check", "--exit-code", "--refresh"]);
    assert_eq!(o.status.code(), Some(10), "{}", text(&o));
    assert!(text(&o).contains("carton 0.1.2 is out"), "{}", text(&o));
    let after = Snapshot::take(&fx.cigscript());
    let changed: Vec<String> = before
        .diff(&after)
        .into_iter()
        .filter(|d| !d.contains("installer/cache/"))
        .collect();
    assert!(
        changed.is_empty(),
        "--check wrote beyond the manifest cache: {changed:?}"
    );
    assert!(which_cig(&fx).contains("cig 1.20.0"));
    // without --exit-code a newer carton is still exit 0
    let o = cigi(&fx, &["update", "--check", "--refresh"]);
    ok(&o);
    // --exit-code alone is a usage error
    let o = cigi(&fx, &["update", "--exit-code"]);
    assert_eq!(o.status.code(), Some(3), "{}", text(&o));
}

#[test]
fn a3_update_check_off_sends_nothing() {
    let mut fx = fixture();
    two(&mut fx, "plan: 1 op");
    let server = fx.serve();
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    server.clear();
    let o = fx
        .cigi_with_mirror(&server.url())
        .env("CIGI_UPDATE_CHECK", "off")
        .args(["--yes", "update"])
        .output()
        .unwrap();
    ok(&o);
    assert!(text(&o).contains("update checks are off"), "{}", text(&o));
    assert!(server.requests().is_empty(), "{:?}", server.paths());
}

#[test]
fn a3_rollback_one_tool() {
    let mut fx = fixture();
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.16.0")]);
    fx.carton("0.2.0", &[("cig", "1.21.0"), ("cigo", "1.17.0")]);
    fx.publish("stable");
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    ok(&cigi(&fx, &["update", "--no-trial"]));
    assert_eq!(
        out(&fx.run_shim("cig", &fx.project, &["--version"])),
        "cig 1.21.0"
    );
    let o = cigi(&fx, &["rollback", "cig"]);
    ok(&o);
    assert!(
        text(&o).contains("rolled cig back to 1.20.0"),
        "{}",
        text(&o)
    );
    assert!(text(&o).contains("a mix"), "{}", text(&o));
    // cig is the old build, cigo is still the new carton's
    assert_eq!(
        out(&fx.run_shim("cig", &fx.project, &["--version"])),
        "cig 1.20.0"
    );
    assert!(which_cig(&fx).contains("cig 1.20.0"));
    let o = fx.cigi().args(["which", "cigo"]).output().unwrap();
    assert!(text(&o).contains("cigo 1.17.0"), "{}", text(&o));
    let st = read(&fx.cigscript().join("installer/state.json"));
    assert!(st.contains("rollback cig"), "recorded as a stick: {st}");
    // a tool the previous carton does not have
    let o = cigi(&fx, &["rollback", "cigcompile"]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(text(&o).contains("E1852"), "{}", text(&o));
}

#[cfg(unix)]
#[test]
fn a3_rollback_verifies_the_old_toolchain_first() {
    use std::os::unix::fs::PermissionsExt;
    let mut fx = fixture();
    two(&mut fx, "plan: 1 op");
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    ok(&cigi(&fx, &["update", "--no-trial"]));
    let dir = fx.cigscript().join("toolchains/cig/1.20.0/bin");
    let bin = dir.join("cig");
    let store = fx.cigscript().join("store");
    // a file gone from the old toolchain is put back from the store before the flip
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::remove_file(&bin).unwrap();
    let o = cigi(&fx, &["rollback"]);
    ok(&o);
    assert_eq!(
        out(&fx.run_shim("cig", &fx.project, &["--version"])),
        "cig 1.20.0"
    );

    // damaged beyond repair (the store object is gone too): refused, E1831, nothing flipped
    ok(&cigi(&fx, &["update", "--no-trial"]));
    std::fs::remove_file(&bin).unwrap();
    for e in walk(&store) {
        if std::fs::read(&e).is_ok_and(|b| b.starts_with(b"#!/bin/sh\n# fake cig 1.20.0")) {
            std::fs::set_permissions(e.parent().unwrap(), std::fs::Permissions::from_mode(0o755))
                .unwrap();
            std::fs::remove_file(&e).unwrap();
        }
    }
    let idx = index(&fx);
    let o = cigi(&fx, &["rollback"]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(text(&o).contains("E1831"), "{}", text(&o));
    assert_eq!(index(&fx), idx);
    assert_eq!(state(&fx)["machine_carton"], "0.1.1");
}

#[cfg(unix)]
fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut v = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                v.extend(walk(&p));
            } else {
                v.push(p);
            }
        }
    }
    v
}

#[test]
fn a3_notice_once_a_day_on_a_terminal_only() {
    let mut fx = fixture();
    two(&mut fx, "plan: 1 op");
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    // the install cached a manifest that has 0.1.1: a command on a terminal says so, once
    let (code, t) = on_tty(&fx, "list", &[], &[]);
    assert_eq!(code, 0, "{t}");
    assert!(t.contains("carton 0.1.1 is out"), "{t}");
    assert!(t.contains("ciginstaller update"), "{t}");
    let notice = read(&fx.cigscript().join("installer/notice.json"));
    assert!(notice.contains("\"carton\": \"0.1.1\""), "{notice}");
    let (_, t) = on_tty(&fx, "list", &[], &[]);
    assert!(!t.contains("is out"), "a second notice the same day: {t}");
    // no terminal: never
    std::fs::remove_file(fx.cigscript().join("installer/notice.json")).unwrap();
    let o = raw(&fx, &["list"]);
    assert!(!text(&o).contains("is out"), "{}", text(&o));
    // and `auto_update = off` keeps a terminal quiet too
    let (_, t) = on_tty(&fx, "list", &[], &[("CIGI_AUTO_UPDATE", "off")]);
    assert!(!t.contains("is out"), "{t}");
    assert!(!fx.cigscript().join("installer/notice.json").exists());
}

/// Install 0.1.0, update to 0.1.1: the setup of the undo and kill tests.
fn updated() -> Fixture {
    let mut fx = fixture();
    two(&mut fx, "plan: 1 op");
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    ok(&cigi(&fx, &["update", "--no-trial"]));
    fx
}

fn cig_dirs(fx: &Fixture) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(fx.cigscript().join("toolchains/cig"))
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// The undo of an update is for a rollback right after it: `use` back and forth in between makes
/// the next rollback a plain switch that deletes nothing.
#[test]
fn a3_rollback_undoes_only_the_update_right_before_it() {
    let fx = updated();
    ok(&cigi(&fx, &["use", "0.1.0"]));
    ok(&cigi(&fx, &["use", "0.1.1"]));
    let o = cigi(&fx, &["rollback"]);
    ok(&o);
    assert!(text(&o).contains("stays installed"), "{}", text(&o));
    assert_eq!(state(&fx)["machine_carton"], "0.1.0");
    assert_eq!(state(&fx)["previous"]["machine_carton"], "0.1.1");
    assert!(state(&fx)["cartons"].get("0.1.1").is_some());
    assert_eq!(cig_dirs(&fx), ["1.20.0", "1.21.0"], "nothing was deleted");
    ok(&cigi(&fx, &["use", "0.1.1"]));
    // and the plain case still undoes: the toolchain is gone once the rollback is done
    let fx = updated();
    ok(&cigi(&fx, &["rollback"]));
    assert_eq!(cig_dirs(&fx), ["1.20.0"]);
    assert!(state(&fx)["cartons"].get("0.1.1").is_none());
}

fn crashed(o: &Output) -> bool {
    o.status.code().is_none()
}

/// `update` killed at every fault point: the next command recovers, and the machine is wholly the
/// old carton or wholly the new one, never between.
#[test]
fn a3_update_survives_a_kill_at_every_point() {
    let phases = [
        "mkdir", "store", "link", "move", "write", "block", "flip", "flipped", "commit",
    ];
    let (mut crashes, mut pre, mut post) = (0, 0, 0);
    for phase in phases {
        for n in 1..60 {
            let mut fx = fixture();
            two(&mut fx, "plan: 1 op");
            ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
            let o = fx
                .cigi()
                .env("CIGI_ALLOW_SINGLE_SOURCE", "true")
                .env("CIGI_FAULT", format!("{phase}:{n}"))
                .args(["--yes", "update", "--no-trial"])
                .output()
                .unwrap();
            if !crashed(&o) {
                break;
            }
            crashes += 1;
            ok(&cigi(&fx, &["update", "--dry-run"]));
            let machine = state(&fx)["machine_carton"].as_str().unwrap().to_string();
            let said = out(&fx.run_shim("cig", &fx.project, &["--version"]));
            match machine.as_str() {
                "0.1.0" => {
                    pre += 1;
                    assert_eq!(said, "cig 1.20.0", "{phase}:{n}");
                    assert_eq!(cig_dirs(&fx), ["1.20.0"], "{phase}:{n}");
                }
                "0.1.1" => {
                    post += 1;
                    assert_eq!(said, "cig 1.21.0", "{phase}:{n}");
                    assert!(index(&fx).contains("machine\t0.1.1"), "{phase}:{n}");
                }
                other => panic!("{phase}:{n}: machine carton {other}"),
            }
            let o = fx.run_shim("cigo", &fx.project, &[]);
            assert!(
                out(&o).starts_with("cigo 1.17.0"),
                "{phase}:{n}: {}",
                text(&o)
            );
        }
    }
    assert!(crashes >= 10, "only {crashes} fault points reached");
    assert!(pre > 0 && post > 0, "pre {pre}, post {post}");
}

/// The undo of an update killed at every point: the machine is wholly the new carton or wholly the
/// old one, and the shims run what the state says.
#[test]
fn a3_rollback_undo_survives_a_kill_at_every_point() {
    let phases = [
        "mkdir", "store", "link", "move", "write", "block", "flip", "flipped", "commit",
    ];
    let (mut crashes, mut pre, mut post) = (0, 0, 0);
    for phase in phases {
        for n in 1..60 {
            let fx = updated();
            let o = fx
                .cigi()
                .env("CIGI_ALLOW_SINGLE_SOURCE", "true")
                .env("CIGI_FAULT", format!("{phase}:{n}"))
                .args(["--yes", "rollback"])
                .output()
                .unwrap();
            if !crashed(&o) {
                break;
            }
            crashes += 1;
            ok(&cigi(&fx, &["update", "--dry-run"]));
            let machine = state(&fx)["machine_carton"].as_str().unwrap().to_string();
            let said = out(&fx.run_shim("cig", &fx.project, &["--version"]));
            match machine.as_str() {
                "0.1.1" => {
                    pre += 1;
                    assert_eq!(said, "cig 1.21.0", "{phase}:{n}");
                    assert!(cig_dirs(&fx).contains(&"1.21.0".to_string()), "{phase}:{n}");
                }
                "0.1.0" => {
                    post += 1;
                    assert_eq!(said, "cig 1.20.0", "{phase}:{n}");
                }
                other => panic!("{phase}:{n}: machine carton {other}"),
            }
            let o = fx.run_shim("cigo", &fx.project, &[]);
            assert!(
                out(&o).starts_with("cigo 1.17.0"),
                "{phase}:{n}: {}",
                text(&o)
            );
        }
    }
    assert!(crashes >= 5, "only {crashes} fault points reached");
    assert!(pre > 0 && post > 0, "pre {pre}, post {post}");
}
