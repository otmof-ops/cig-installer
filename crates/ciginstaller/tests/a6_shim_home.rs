// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! #46 and #47: a shim knows its own home, and a moved or restored home never runs the old
//! home's binaries. Each test installs into a home that is not `~/.cigscript` and runs the
//! shims with nothing but `PATH` (no `CIGSCRIPT_HOME`), the way the install's own advice leaves
//! a shell. Every child process has a hard timeout.
#![cfg(unix)]

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use cigi_testkit::Fixture;
use common::{cigi, fixture, ok, text};

const LIMIT: Duration = Duration::from_secs(60);

/// Run a command with a hard timeout; the child is killed when it is over.
fn timed(mut c: Command) -> Output {
    c.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut tries = 0;
    let mut child = loop {
        match c.spawn() {
            Err(e) if e.raw_os_error() == Some(26) && tries < 100 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(5));
            }
            r => break r.expect("start the child"),
        }
    };
    let start = Instant::now();
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if start.elapsed() > LIMIT {
            let _ = child.kill();
            panic!("a child ran past {LIMIT:?} and was killed");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn setup() -> Fixture {
    let mut fx = fixture();
    fx.carton(
        "0.1.0",
        &[
            ("cig", "1.20.0"),
            ("cigo", "1.17.0"),
            ("cigcompile", "0.2.0"),
        ],
    );
    fx.carton("0.2.0", &[("cig", "1.21.0")]);
    fx.publish("stable");
    fx
}

fn dir(fx: &Fixture, name: &str) -> PathBuf {
    let d = fx.tmp.path().join("homes").join(name);
    std::fs::create_dir_all(d.parent().unwrap()).unwrap();
    d
}

/// `install --home <home>` of a carton, the way the person did it.
fn install_in(fx: &Fixture, home: &Path, carton: &str) {
    ok(&cigi(
        fx,
        &[
            "install",
            "carton",
            "--carton",
            carton,
            "--no-path",
            "--home",
            &home.display().to_string(),
        ],
    ));
}

/// A shim run the way a new shell leaves it: `PATH` with the home's `bin` and nothing else about
/// the home. `env` adds variables.
fn shim(fx: &Fixture, home: &Path, name: &str, env: &[(&str, &Path)], verbose: bool) -> Output {
    let mut c = Command::new(home.join("bin").join(name));
    c.env_clear()
        .env("HOME", &fx.user_home)
        .env("LANG", "C")
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", home.join("bin").display()),
        )
        .current_dir(&fx.project);
    for (k, v) in env {
        c.env(k, v);
    }
    if verbose {
        c.env("CIGI_SHIM_VERBOSE", "1");
    }
    timed(c)
}

/// The installer against `home`, with `--home`.
fn run_cigi(fx: &Fixture, home: &Path, args: &[&str]) -> Output {
    let mut c = fx.cigi();
    c.env("CIGI_ALLOW_SINGLE_SOURCE", "true")
        .arg("--yes")
        .arg("--home")
        .arg(home)
        .args(args);
    timed(c)
}

fn cig_target(home: &Path, ver: &str) -> PathBuf {
    home.join(format!("toolchains/cig/{ver}/bin/cig"))
}

/// What a person does to swap a build: a new file at the same path (a new inode).
fn swap(p: &Path, body: &str) {
    std::fs::remove_file(p).unwrap();
    std::fs::write(p, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(p, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
}

fn cp_a(from: &Path, to: &Path) {
    let st = Command::new("cp")
        .arg("-a")
        .arg(from)
        .arg(to)
        .status()
        .unwrap();
    assert!(st.success());
}

#[test]
fn shims_work_from_a_custom_home_on_path_alone() {
    let fx = setup();
    let home = dir(&fx, "custom");
    let o = cigi(
        &fx,
        &[
            "install",
            "carton",
            "--carton",
            "0.1.0",
            "--home",
            &home.display().to_string(),
        ],
    );
    ok(&o);
    // the advice is the PATH line alone: it no longer asks for CIGSCRIPT_HOME
    assert!(!text(&o).contains("CIGSCRIPT_HOME"), "{}", text(&o));
    for (name, want) in [
        ("cig", "cig 1.20.0"),
        ("cigo", "cigo 1.17.0"),
        ("cigcompile", "cigcompile 0.2.0"),
    ] {
        let o = shim(&fx, &home, name, &[], false);
        let t = text(&o);
        assert_eq!(o.status.code(), Some(0), "{name}: {t}");
        assert!(t.contains(want), "{name}: {t}");
    }
}

#[test]
fn cigo_finds_cig_through_the_same_home() {
    let fx = setup();
    let home = dir(&fx, "custom");
    install_in(&fx, &home, "0.1.0");
    // nothing at ~/.cigscript and no CIGSCRIPT_HOME: cigo runs `cig` from PATH, which is this
    // home's anchored shim
    assert!(!fx.cigscript().exists());
    let o = shim(&fx, &home, "cigo", &[], false);
    let t = text(&o);
    assert_eq!(o.status.code(), Some(0), "{t}");
    assert!(t.contains("cigo 1.17.0 uses cig 1.20.0"), "{t}");
}

#[test]
fn a_shim_runs_its_own_homes_build_whatever_cigscript_home_says() {
    let fx = setup();
    let (a, b) = (dir(&fx, "a"), dir(&fx, "b"));
    install_in(&fx, &a, "0.1.0");
    install_in(&fx, &b, "0.2.0");
    // a's shim with CIGSCRIPT_HOME at b (decision 0005): a's build runs, and the shim says both
    // homes when asked
    let o = shim(&fx, &a, "cig", &[("CIGSCRIPT_HOME", &b)], true);
    let t = text(&o);
    assert_eq!(o.status.code(), Some(0), "{t}");
    assert!(t.contains("cig 1.20.0"), "a's build runs: {t}");
    let (sa, sb) = (a.display().to_string(), b.display().to_string());
    assert!(t.contains(&sa) && t.contains(&sb), "names both homes: {t}");
    assert!(t.contains("the home it lives in wins"), "{t}");
    // CIGSCRIPT_HOME at an empty folder (cig's box points it at the run's inbox): the same
    let empty = dir(&fx, "empty");
    std::fs::create_dir_all(&empty).unwrap();
    let o = shim(&fx, &a, "cig", &[("CIGSCRIPT_HOME", &empty)], false);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    assert!(text(&o).contains("cig 1.20.0"), "{}", text(&o));
    // without the variable a's own build runs, and nothing is said about a disagreement
    let o = shim(&fx, &a, "cig", &[], true);
    assert!(text(&o).contains("cig 1.20.0"), "{}", text(&o));
    assert!(!text(&o).contains("CIGSCRIPT_HOME is"), "{}", text(&o));
    // the installer's own copy in a's bin works on a, with CIGSCRIPT_HOME at the empty folder
    let mut c = Command::new(a.join("bin/ciginstaller"));
    c.env_clear()
        .env("HOME", &fx.user_home)
        .env("LANG", "C")
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .env("CIGI_ROUTES", "none")
        .env("CIGI_NO_REGISTRY_PATH", "1")
        .env("CIGSCRIPT_HOME", &empty)
        .env("CIGI_MIRROR", fx.mirror_file_url())
        .args(["--plain", "status"])
        .current_dir(&fx.project);
    let o = timed(c);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    assert!(text(&o).contains("0.1.0"), "a's carton: {}", text(&o));
    assert!(
        std::fs::read_dir(&empty).unwrap().next().is_none(),
        "nothing was written to CIGSCRIPT_HOME's folder"
    );
}

#[test]
fn a_binary_in_no_home_follows_cigscript_home_and_doctor_names_both() {
    let fx = setup();
    let b = dir(&fx, "b");
    install_in(&fx, &b, "0.2.0");
    let sb = b.display().to_string();
    // a ciginstaller in a `bin` that is not a home, told to work on b
    let x = dir(&fx, "x");
    std::fs::create_dir_all(x.join("bin")).unwrap();
    let real = x.join("bin/ciginstaller");
    std::fs::copy(cigi_testkit::binary_path("ciginstaller"), &real).unwrap();
    let run = |args: &[&str]| {
        let mut c = Command::new(&real);
        c.env_clear()
            .env("HOME", &fx.user_home)
            .env("LANG", "C")
            .env("NO_COLOR", "1")
            .env("TERM", "dumb")
            .env("CIGI_ROUTES", "none")
            .env("CIGI_NO_REGISTRY_PATH", "1")
            .env("CIGSCRIPT_HOME", &b)
            .env("CIGI_MIRROR", fx.mirror_file_url())
            .args(args)
            .current_dir(&fx.project);
        text(&timed(c))
    };
    let t = run(&["--plain", "status"]);
    assert!(t.contains("0.2.0"), "b's carton: {t}");
    let t = run(&["--plain", "doctor"]);
    assert!(
        t.contains(&x.display().to_string()) && t.contains(&sb),
        "doctor names both: {t}"
    );
    assert!(t.contains("CIGSCRIPT_HOME"), "{t}");
}

#[test]
fn e1840_never_says_missing_when_it_is_beside_the_shim() {
    let fx = setup();
    let home = dir(&fx, "custom");
    install_in(&fx, &home, "0.1.0");
    // a copy of the shim and the installer in a `bin` that is not a home: CIGSCRIPT_HOME chooses,
    // and it names an empty folder
    let y = dir(&fx, "y");
    std::fs::create_dir_all(y.join("bin")).unwrap();
    std::fs::copy(home.join("bin/cig"), y.join("bin/cig")).unwrap();
    std::fs::copy(home.join("bin/ciginstaller"), y.join("bin/ciginstaller")).unwrap();
    let empty = dir(&fx, "empty");
    std::fs::create_dir_all(&empty).unwrap();
    let o = shim(&fx, &y, "cig", &[("CIGSCRIPT_HOME", &empty)], false);
    let t = text(&o);
    assert_eq!(o.status.code(), Some(127), "{t}");
    assert!(t.contains("E1840"), "{t}");
    assert!(!t.to_lowercase().contains("missing"), "{t}");
    assert!(
        t.contains(&empty.display().to_string()),
        "the home searched: {t}"
    );
    assert!(
        t.contains(&y.join("bin").display().to_string()),
        "the shim's own folder: {t}"
    );
    // and when there truly is no ciginstaller, it says so
    std::fs::remove_file(y.join("bin/ciginstaller")).unwrap();
    let o = shim(&fx, &y, "cig", &[("CIGSCRIPT_HOME", &empty)], false);
    assert!(text(&o).contains("no ciginstaller"), "{}", text(&o));
}

#[test]
fn moved_home_runs_its_own_builds() {
    let fx = setup();
    let old = dir(&fx, "old");
    install_in(&fx, &old, "0.1.0");
    // a plain move keeps every device and inode: it just works, and runs its own build
    let renamed = dir(&fx, "renamed");
    std::fs::rename(&old, &renamed).unwrap();
    let o = shim(&fx, &renamed, "cig", &[], false);
    assert!(text(&o).contains("cig 1.20.0"), "{}", text(&o));
    // a copy gets new inodes: the shims refuse until repair, and never run the old path's file
    let copy = dir(&fx, "copy");
    cp_a(&renamed, &copy);
    swap(
        &cig_target(&renamed, "1.20.0"),
        "echo FAKE CIG FROM THE OLD HOME",
    );
    let o = shim(&fx, &copy, "cig", &[], false);
    let t = text(&o);
    assert_eq!(o.status.code(), Some(127), "{t}");
    assert!(
        t.contains("E1903") && t.contains("ciginstaller repair"),
        "{t}"
    );
    assert!(!t.contains("FAKE"), "{t}");
    // doctor names the cause
    let d = run_cigi(&fx, &copy, &["--plain", "doctor"]);
    let dt = text(&d);
    assert!(
        dt.contains("moved from") && dt.contains(&old.display().to_string()),
        "{dt}"
    );
    ok(&run_cigi(&fx, &copy, &["repair"]));
    let o = shim(&fx, &copy, "cig", &[], false);
    assert!(text(&o).contains("cig 1.20.0"), "{}", text(&o));
    assert!(!text(&o).contains("FAKE"), "{}", text(&o));
    // the old home's fake stays the old home's
    assert!(std::fs::read_to_string(cig_target(&renamed, "1.20.0"))
        .unwrap()
        .contains("FAKE"));
}

#[test]
fn shim_refuses_a_target_outside_its_home() {
    let fx = setup();
    let home = dir(&fx, "custom");
    install_in(&fx, &home, "0.1.0");
    let evil = dir(&fx, "evil").join("cig");
    std::fs::create_dir_all(evil.parent().unwrap()).unwrap();
    swap_new(&evil, "echo EVIL RAN");
    let idx = home.join("installer/shim-index");
    let original = std::fs::read_to_string(&idx).unwrap();
    // the newer form and the older absolute form, both leading out
    for (head, target) in [
        ("cigi-index 2", evil.display().to_string()),
        ("cigi-index 1", evil.display().to_string()),
        ("cigi-index 2", "../../evil/cig".to_string()),
    ] {
        let edited: Vec<String> = original
            .lines()
            .enumerate()
            .map(|(i, l)| {
                if i == 0 {
                    head.to_string()
                } else if l.starts_with("carton\t0.1.0\tcig\t") {
                    format!("carton\t0.1.0\tcig\t{target}")
                } else {
                    l.to_string()
                }
            })
            .collect();
        std::fs::write(&idx, edited.join("\n") + "\n").unwrap();
        let o = shim(&fx, &home, "cig", &[], false);
        let t = text(&o);
        assert_eq!(o.status.code(), Some(127), "{target}: {t}");
        assert!(!t.contains("EVIL"), "nothing ran: {t}");
        assert!(t.contains("E1902") || t.contains("E1844"), "{target}: {t}");
        if !target.contains("..") {
            assert!(
                t.contains("E1902") && t.contains("ciginstaller repair"),
                "{t}"
            );
        }
    }
    // repair writes the index again
    ok(&run_cigi(&fx, &home, &["repair"]));
    assert!(text(&shim(&fx, &home, "cig", &[], false)).contains("cig 1.20.0"));
}

fn swap_new(p: &Path, body: &str) {
    std::fs::write(p, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(p, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
}

#[test]
fn shim_refuses_a_swapped_target() {
    let fx = setup();
    let home = dir(&fx, "custom");
    install_in(&fx, &home, "0.1.0");
    assert!(text(&shim(&fx, &home, "cig", &[], false)).contains("cig 1.20.0"));
    swap(&cig_target(&home, "1.20.0"), "echo SWAPPED");
    let o = shim(&fx, &home, "cig", &[], false);
    let t = text(&o);
    assert_eq!(o.status.code(), Some(127), "{t}");
    assert!(
        t.contains("E1903") && t.contains("ciginstaller repair"),
        "{t}"
    );
    assert!(!t.contains("SWAPPED"), "{t}");
    ok(&run_cigi(&fx, &home, &["repair"]));
    let o = shim(&fx, &home, "cig", &[], false);
    assert!(text(&o).contains("cig 1.20.0"), "{}", text(&o));
}

#[test]
fn restored_backup_works() {
    let fx = setup();
    let home = dir(&fx, "orig");
    install_in(&fx, &home, "0.1.0");
    let tar = fx.tmp.path().join("backup.tar");
    let parent = home.parent().unwrap();
    let st = Command::new("tar")
        .arg("-C")
        .arg(parent)
        .arg("-cf")
        .arg(&tar)
        .arg("orig")
        .status()
        .unwrap();
    assert!(st.success());
    std::fs::remove_dir_all(&home).unwrap();
    let elsewhere = fx.tmp.path().join("restored-here");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let st = Command::new("tar")
        .arg("-C")
        .arg(&elsewhere)
        .arg("-xf")
        .arg(&tar)
        .status()
        .unwrap();
    assert!(st.success());
    let restored = elsewhere.join("orig");
    // new device and inode numbers: the shim refuses, and repair makes the home whole again
    let o = shim(&fx, &restored, "cig", &[], false);
    assert!(text(&o).contains("E1903"), "{}", text(&o));
    let r = run_cigi(&fx, &restored, &["repair"]);
    ok(&r);
    for (name, want) in [
        ("cig", "cig 1.20.0"),
        ("cigo", "cigo 1.17.0 uses cig 1.20.0"),
    ] {
        let o = shim(&fx, &restored, name, &[], false);
        assert_eq!(o.status.code(), Some(0), "{name}: {}", text(&o));
        assert!(text(&o).contains(want), "{name}: {}", text(&o));
    }
    // repaired once, it stays working
    let d = run_cigi(&fx, &restored, &["--plain", "doctor"]);
    assert!(!text(&d).contains("moved from"), "{}", text(&d));
}
