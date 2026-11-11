// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! #7: the cigo and CigCompile trials in a real update, `auto_update = security | all` through the
//! trial with a rollback, the machine's lock, and the daily schedule written (never loaded).

#![cfg(unix)]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use cigi_testkit::Fixture;
use common::{cigi, fixture, ok, text};

fn read(p: &std::path::Path) -> String {
    std::fs::read_to_string(p).unwrap_or_default()
}

fn machine_carton(fx: &Fixture) -> String {
    let v: serde_json::Value =
        serde_json::from_str(&read(&fx.cigscript().join("installer/state.json"))).unwrap();
    v["machine_carton"].as_str().unwrap_or("").to_string()
}

fn memory(fx: &Fixture) -> serde_json::Value {
    serde_json::from_str(&read(&fx.cigscript().join("installer/auto-update.json")))
        .unwrap_or_default()
}

/// Forget the last attempt, as a day going by does.
fn a_day_later(fx: &Fixture) {
    let p = fx.cigscript().join("installer/auto-update.json");
    let mut v = memory(fx);
    if let Some(o) = v.as_object_mut() {
        o.insert("last_attempt".into(), 0.into());
    }
    std::fs::write(p, serde_json::to_string(&v).unwrap()).unwrap();
}

fn scripts(fx: &Fixture, n: usize) {
    for i in 0..n {
        std::fs::write(
            fx.project.join(format!("s{i}.cig")),
            format!("exhale \"script {i}\"\n"),
        )
        .unwrap();
    }
}

fn layer(fx: &Fixture, body: &str) {
    std::fs::create_dir_all(fx.machine_root()).unwrap();
    let p = fx.machine_root().join("config.toml");
    std::fs::write(&p, body).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
}

fn unit_dir(fx: &Fixture) -> PathBuf {
    fx.user_home.join(".config/systemd/user")
}

/// The files the daily schedule is written to here: a launchd agent on macOS, a systemd user
/// service and timer on Linux and the other unixes (`schedule::Os::host`).
fn schedule_files(fx: &Fixture) -> Vec<PathBuf> {
    if cfg!(target_os = "macos") {
        vec![fx
            .user_home
            .join("Library/LaunchAgents/io.github.otmof-ops.cigi-update.plist")]
    } else {
        vec![
            unit_dir(fx).join("cigi-update.service"),
            unit_dir(fx).join("cigi-update.timer"),
        ]
    }
}

fn no_schedule(fx: &Fixture) -> bool {
    schedule_files(fx).iter().all(|f| !f.exists())
}

/// What the record-only seam logs when the schedule is turned on, and when it is turned off.
fn enable_lines() -> &'static [&'static str] {
    if cfg!(target_os = "macos") {
        &["launchctl bootstrap gui/{uid} "]
    } else {
        &[
            "systemctl --user daemon-reload",
            "systemctl --user enable --now cigi-update.timer",
        ]
    }
}

fn disable_line() -> &'static str {
    if cfg!(target_os = "macos") {
        "launchctl bootout gui/{uid}/io.github.otmof-ops.cigi-update"
    } else {
        "systemctl --user disable --now cigi-update.timer"
    }
}

/// The schedule runs `update --auto`, at low priority, daily.
fn the_schedule_runs_update_auto(fx: &Fixture) {
    if cfg!(target_os = "macos") {
        let plist = read(&schedule_files(fx)[0]);
        assert!(
            plist.contains("<string>update</string>") && plist.contains("<string>--auto</string>"),
            "{plist}"
        );
        assert!(
            plist.contains("<key>StartInterval</key>")
                && plist.contains("<key>LowPriorityIO</key>"),
            "{plist}"
        );
    } else {
        let service = read(&unit_dir(fx).join("cigi-update.service"));
        let timer = read(&unit_dir(fx).join("cigi-update.timer"));
        assert!(service.contains("ConditionACPower=true"), "{service}");
        assert!(service.contains("update\" \"--auto\""), "{service}");
        assert!(timer.contains("RandomizedDelaySec=7200") && timer.contains("OnCalendar=daily"));
    }
}

fn enable_log(fx: &Fixture) -> String {
    read(&fx.cigscript().join("installer/schedule-enable.log"))
}

// ------------------------------------------------------------------------------- the trials

#[test]
fn a8_cigo_trial_runs_in_an_update_and_a_changed_check_keeps_the_old_carton() {
    let mut fx = fixture();
    fx.product("cig", "1.20.0").dry_run_output("plan: 1 op");
    fx.product("cigo", "1.17.0");
    fx.product("cigo", "1.18.0")
        .shell("if [ \"$1\" = check ]; then echo 'E999: a new complaint' >&2; exit 1; fi");
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.17.0")]);
    fx.carton("0.1.1", &[("cig", "1.20.0"), ("cigo", "1.18.0")]);
    fx.publish("stable");
    std::fs::write(
        fx.project.join("cig-tasks.json"),
        "{\"tasks\":{\"build\":{}}}",
    )
    .unwrap();
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));

    let o = cigi(&fx, &["update", "--trial-only"]);
    ok(&o);
    let t = text(&o);
    assert!(t.contains("different: cigo check: exit 0 -> 1"), "{t}");
    assert!(t.contains("+E999: a new complaint"), "{t}");
    // the dry runs of the chains were compared too and agree
    assert!(
        t.contains("3 script(s) compared, 2 the same, 1 different"),
        "{t}"
    );

    let o = cigi(&fx, &["update"]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(text(&o).contains("E1851"), "{}", text(&o));
    assert_eq!(machine_carton(&fx), "0.1.0");
}

#[test]
fn a8_cigcompile_trial_tests_the_persons_programs_with_each_build() {
    let mut fx = fixture();
    fx.product("cig", "1.20.0");
    fx.product("cigcompile", "0.2.0");
    fx.product("cigcompile", "0.3.0")
        .shell("if [ \"$1\" = test ]; then echo 'DIFFERS  ./s0.cig'; exit 1; fi");
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigcompile", "0.2.0")]);
    fx.carton("0.1.1", &[("cig", "1.20.0"), ("cigcompile", "0.3.0")]);
    fx.publish("stable");
    scripts(&fx, 1);
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));

    let o = cigi(&fx, &["update", "--trial-only"]);
    ok(&o);
    let t = text(&o);
    assert!(t.contains("s0.cig (CigCompile test)"), "{t}");
    assert!(t.contains("+DIFFERS  ./s0.cig"), "{t}");
    // the programs were built in scratch: the project holds only its own script
    let names: Vec<String> = std::fs::read_dir(&fx.project)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(names, ["s0.cig"], "{names:?}");
}

// ------------------------------------------------------------------------------ auto_update

fn cartons_three(fx: &mut Fixture, new_plan: &str) {
    fx.product("cig", "1.20.0").dry_run_output("plan: 1 op");
    fx.product("cig", "1.21.0").dry_run_output(new_plan);
    fx.product("cig", "1.22.0").dry_run_output(new_plan);
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.17.0")]);
    fx.carton("0.1.1", &[("cig", "1.21.0"), ("cigo", "1.17.0")]);
    fx.carton("0.1.2", &[("cig", "1.22.0"), ("cigo", "1.17.0")]);
}

#[test]
fn a8_auto_all_installs_the_newest_carton_through_the_trial() {
    let mut fx = fixture();
    cartons_three(&mut fx, "plan: 1 op");
    fx.publish("stable");
    scripts(&fx, 2);
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));

    // notify (the default) installs nothing, however often it runs
    let o = cigi(&fx, &["update", "--auto"]);
    ok(&o);
    assert!(text(&o).contains("auto_update = notify"), "{}", text(&o));
    assert_eq!(machine_carton(&fx), "0.1.0");

    ok(&cigi(&fx, &["config", "set", "auto_update", "all"]));
    let o = cigi(&fx, &["update", "--auto"]);
    ok(&o);
    let t = text(&o);
    assert!(t.contains("updated carton 0.1.0 to 0.1.2"), "{t}");
    assert!(t.contains("trial: 2 script(s) compared, 2 the same"), "{t}");
    assert!(
        t.contains("installed automatically: auto_update = all"),
        "{t}"
    );
    assert_eq!(machine_carton(&fx), "0.1.2");
    assert_eq!(memory(&fx)["result"], "updated");

    // at most daily: another run at once does nothing
    let o = cigi(&fx, &["update", "--auto"]);
    ok(&o);
    assert!(
        text(&o).contains("tried less than a day ago"),
        "{}",
        text(&o)
    );
}

#[test]
fn a8_auto_failed_trial_rolls_back_says_so_and_does_not_retry_the_same_carton() {
    let mut fx = fixture();
    cartons_three(&mut fx, "plan: 2 ops");
    fx.publish("stable");
    scripts(&fx, 2);
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    ok(&cigi(&fx, &["config", "set", "auto_update", "all"]));

    let o = cigi(&fx, &["update", "--auto"]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    let t = text(&o);
    assert!(t.contains("E1851"), "{t}");
    assert!(
        t.contains(
            "automatic update to carton 0.1.2 failed and was rolled back; carton 0.1.0 stays"
        ),
        "{t}"
    );
    assert_eq!(machine_carton(&fx), "0.1.0");
    let m = memory(&fx);
    assert_eq!(m["result"], "rolled_back");
    assert_eq!(m["shown"], false, "the next terminal says it once");
    assert_eq!(m["failed"]["carton"], "0.1.2");

    // a day later the same carton is not tried again
    a_day_later(&fx);
    let o = cigi(&fx, &["update", "--auto"]);
    ok(&o);
    assert!(
        text(&o).contains("not tried again for 7 days"),
        "{}",
        text(&o)
    );
    assert_eq!(machine_carton(&fx), "0.1.0");
}

#[test]
fn a8_auto_security_takes_only_a_carton_that_clears_a_security_advisory() {
    let mut fx = fixture();
    cartons_three(&mut fx, "plan: 1 op");
    // 0.1.1 still carries cig 1.21.0, which the advisory names: only 0.1.2 clears it
    fx.advisory("CIGI-2026-0101", "security", "cig <1.22.0", "1.22.0");
    fx.publish("stable");
    scripts(&fx, 1);
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    ok(&cigi(&fx, &["config", "set", "auto_update", "security"]));

    let o = cigi(&fx, &["update", "--auto"]);
    ok(&o);
    let t = text(&o);
    assert!(t.contains("updated carton 0.1.0 to 0.1.2"), "{t}");
    assert!(t.contains("it clears CIGI-2026-0101"), "{t}");
    assert_eq!(machine_carton(&fx), "0.1.2");
}

#[test]
fn a8_auto_security_ignores_a_bug_advisory_and_a_carton_with_no_advisory() {
    let mut fx = fixture();
    cartons_three(&mut fx, "plan: 1 op");
    fx.advisory("CIGI-2026-0102", "bug", "cig <1.22.0", "1.22.0");
    fx.publish("stable");
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    ok(&cigi(&fx, &["config", "set", "auto_update", "security"]));

    let o = cigi(&fx, &["update", "--auto"]);
    ok(&o);
    assert!(
        text(&o).contains("no newer carton clears a security advisory"),
        "{}",
        text(&o)
    );
    assert_eq!(machine_carton(&fx), "0.1.0");
}

#[test]
fn a8_auto_obeys_the_machine_lock_and_waits_for_mains_power() {
    let mut fx = fixture();
    cartons_three(&mut fx, "plan: 1 op");
    fx.publish("stable");
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    // IT locks auto_update to off: the person's `all` in their own file does not count
    layer(&fx, "auto_update = \"off\"\nlocked = [ \"auto_update\" ]\n");
    let p = fx.home.config_path();
    std::fs::write(&p, "auto_update = all\n").unwrap();
    let o = cigi(&fx, &["update", "--auto"]);
    ok(&o);
    assert!(text(&o).contains("auto_update = off"), "{}", text(&o));
    assert_eq!(machine_carton(&fx), "0.1.0");
    assert!(no_schedule(&fx));

    // IT locks it to `all`: that is obeyed, and on battery nothing runs
    layer(&fx, "auto_update = \"all\"\nlocked = [ \"auto_update\" ]\n");
    let power = fx.tmp.path().join("power");
    let ac = power.join("AC");
    std::fs::create_dir_all(&ac).unwrap();
    std::fs::write(ac.join("type"), "Mains\n").unwrap();
    std::fs::write(ac.join("online"), "0\n").unwrap();
    let o = fx
        .cigi()
        .env("CIGI_ALLOW_SINGLE_SOURCE", "true")
        .env("CIGI_TEST_POWER_ROOT", &power)
        .args(["--yes", "update", "--auto"])
        .output()
        .unwrap();
    ok(&o);
    assert!(text(&o).contains("on battery power"), "{}", text(&o));
    assert_eq!(machine_carton(&fx), "0.1.0");
    // plugged in
    std::fs::write(ac.join("online"), "1\n").unwrap();
    let o = fx
        .cigi()
        .env("CIGI_ALLOW_SINGLE_SOURCE", "true")
        .env("CIGI_TEST_POWER_ROOT", &power)
        .args(["--yes", "update", "--auto"])
        .output()
        .unwrap();
    ok(&o);
    assert_eq!(machine_carton(&fx), "0.1.2", "{}", text(&o));
}

// ------------------------------------------------------------------------------- the schedule

#[test]
fn a8_schedule_is_written_through_the_journal_enabled_by_record_only_and_taken_away() {
    let mut fx = fixture();
    cartons_three(&mut fx, "plan: 1 op");
    fx.publish("stable");
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    // the default (notify) writes nothing
    assert!(no_schedule(&fx));

    let o = cigi(&fx, &["config", "set", "auto_update", "security"]);
    ok(&o);
    assert!(
        text(&o).contains("daily update schedule is in place"),
        "{}",
        text(&o)
    );
    the_schedule_runs_update_auto(&fx);
    // enabling went through the seam: recorded, never run
    let log = enable_log(&fx);
    for line in enable_lines() {
        assert!(log.contains(line), "{log}");
    }
    let state = read(&fx.cigscript().join("installer/state.json"));
    assert!(state.contains("\"schedule\""), "{state}");
    // and it is in the journal
    let hist = cigi(&fx, &["history"]);
    ok(&hist);

    // a second command changes nothing
    let before = enable_log(&fx);
    ok(&cigi(&fx, &["config", "set", "auto_update", "all"]));
    assert_eq!(enable_log(&fx), before, "the files were already in place");

    // off takes it away, and says it was disabled first
    ok(&cigi(&fx, &["config", "set", "auto_update", "off"]));
    assert!(no_schedule(&fx));
    assert!(
        enable_log(&fx).contains(disable_line()),
        "{}",
        enable_log(&fx)
    );
    assert!(!read(&fx.cigscript().join("installer/state.json")).contains("\"schedule\""));
}

#[test]
fn a8_uninstall_all_removes_the_schedule() {
    let mut fx = fixture();
    cartons_three(&mut fx, "plan: 1 op");
    fx.publish("stable");
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    ok(&cigi(&fx, &["config", "set", "auto_update", "all"]));
    assert!(schedule_files(&fx).iter().all(|f| f.exists()));

    let o = cigi(&fx, &["uninstall", "--all", "--yes-irreversible"]);
    ok(&o);
    assert!(no_schedule(&fx), "{}", text(&o));
}
