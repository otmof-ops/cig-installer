// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0
//! Phase 2 review fixes: the resolver, install, `use` and `which` (SPEC 8, 11.2, 12).

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use cigi_testkit::Fixture;
use common::{cigi, fixture, ok, out, text};

/// A shim (or any binary) run with the fixture's environment plus `extra`, no terminal on stdin.
/// Retries "text file busy" (ETXTBSY), which a freshly written binary can hit under parallel tests.
fn run_in(fx: &Fixture, exe: &Path, cwd: &Path, args: &[&str], extra: &[(&str, &str)]) -> Output {
    let mut tries = 0;
    loop {
        let mut c = Command::new(exe);
        c.env_clear();
        for (k, v) in fx.cigi().get_envs() {
            if let Some(v) = v {
                c.env(k, v);
            }
        }
        c.env("CIGI_ALLOW_SINGLE_SOURCE", "true");
        for (k, v) in extra {
            c.env(k, v);
        }
        c.current_dir(cwd).args(args).stdin(Stdio::null());
        match c.output() {
            Err(e) if e.raw_os_error() == Some(26) && tries < 100 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(5));
            }
            r => return r.expect("run"),
        }
    }
}

fn shim(fx: &Fixture, tool: &str, cwd: &Path, extra: &[(&str, &str)]) -> Output {
    run_in(fx, &fx.cigscript().join("bin").join(tool), cwd, &[], extra)
}

fn which(fx: &Fixture, tool: &str, cwd: &Path, extra: &[(&str, &str)]) -> Output {
    let exe = common_installer();
    run_in(fx, &exe, cwd, &["which", tool], extra)
}

fn common_installer() -> PathBuf {
    cigi_testkit::binary_path("ciginstaller")
}

fn project(fx: &Fixture, name: &str, files: &[(&str, &str)]) -> PathBuf {
    let p = fx.tmp.path().join(name);
    std::fs::create_dir_all(&p).unwrap();
    for (f, c) in files {
        std::fs::write(p.join(f), c).unwrap();
    }
    p
}

fn pinned(fx: &Fixture, name: &str, pin: &str) -> PathBuf {
    project(fx, name, &[(".cig-carton", &format!("carton {pin}\n"))])
}

fn install(fx: &Fixture, args: &[&str]) -> Output {
    let mut v = vec!["install"];
    v.extend_from_slice(args);
    cigi(fx, &v)
}

fn two_cartons(fx: &mut Fixture) {
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.16.0")]);
    fx.carton("0.2.0", &[("cig", "1.21.0"), ("cigo", "1.17.0")]);
    fx.publish("stable");
}

/// The tool's own line. On the slow path the installer writes nothing to stdout (it is the
/// tool's), so the tool's line is the only one there.
fn last_line(o: &Output) -> String {
    let text = out(o);
    assert!(
        text.lines().count() <= 1,
        "stdout holds more than the tool's line:\n{text}"
    );
    text
}

fn state_bytes(fx: &Fixture) -> Vec<u8> {
    std::fs::read(fx.cigscript().join("installer/state.json")).unwrap()
}

/// DEV-004 finding 2: carton 0.1.0 {cig 1.5.0, cigar 0.2.0} is the machine's, 0.2.0 {cig 1.20.0,
/// cigar 0.1.0} is on the channel, and the project pins `cig >=1.6.2` and `cigar >=0.2.0`. The
/// slow path used to wait on its own lock for 30 s after the stick fetch and fail with E1834.
#[test]
fn a2r_legacy_two_tool_stick() {
    let mut fx = fixture();
    fx.carton("0.1.0", &[("cig", "1.5.0"), ("cigar", "0.2.0")]);
    fx.carton("0.2.0", &[("cig", "1.20.0"), ("cigar", "0.1.0")]);
    fx.publish("stable");
    ok(&install(&fx, &["carton", "--carton", "0.1.0"]));
    let proj = project(
        &fx,
        "legacy2",
        &[
            (".cig-version", ">=1.6.2\n"),
            (".cigar-version", ">=0.2.0\n"),
        ],
    );
    let t = Instant::now();
    let o = shim(&fx, "cig", &proj, &[("CIGI_PIN_FETCH", "auto")]);
    assert!(t.elapsed() < Duration::from_secs(20), "waited on a lock");
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    let t = text(&o);
    assert!(t.contains("cig 1.20.0 args="), "{t}");
    assert!(!t.contains("E1834"), "{t}");
    // the other tool still comes from the base carton, and nothing is fetched again
    let o = shim(&fx, "cigar", &proj, &[("CIGI_PIN_FETCH", "never")]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    assert!(text(&o).contains("cigar 0.2.0 args="), "{}", text(&o));
    let o = shim(&fx, "cig", &proj, &[("CIGI_PIN_FETCH", "never")]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    let o = which(&fx, "cig", &proj, &[]);
    ok(&o);
    assert!(
        text(&o).contains("cig 1.20.0, from carton 0.2.0 (satisfies"),
        "{}",
        text(&o)
    );
}

/// Finding 3: `use` refuses a partial carton, and one that lacks a tool the machine has.
#[test]
fn a2r_use_refuses_partial() {
    let mut fx = fixture();
    two_cartons(&mut fx);
    fx.carton("0.3.0", &[("cig", "1.22.0")]);
    fx.publish("stable");
    ok(&install(&fx, &["carton", "--carton", "0.1.0"]));
    ok(&install(&fx, &["stick", "cig", "--carton", "0.2.0"]));
    let before = state_bytes(&fx);
    let o = cigi(&fx, &["use", "0.2.0"]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(text(&o).contains("E1817"), "{}", text(&o));
    assert!(
        text(&o).contains("ciginstaller install carton --carton 0.2.0"),
        "{}",
        text(&o)
    );
    assert_eq!(state_bytes(&fx), before, "a refusal changes nothing");
    let o = cigi(&fx, &["carton", "0.2.0"]);
    ok(&o); // `carton <v>` installs the whole carton first
            // the whole carton can be used
    ok(&cigi(&fx, &["use", "0.1.0"]));
    // a carton that has no cigo by design (the manifest's 0.3.0 is cig alone): switched, with a
    // warning that names the stick that keeps cigo
    ok(&install(&fx, &["carton", "--carton", "0.3.0"]));
    let o = cigi(&fx, &["--plain", "use", "0.3.0"]);
    ok(&o);
    assert!(text(&o).contains("has no cigo"), "{}", text(&o));
    assert!(
        text(&o).contains("ciginstaller install stick cigo --carton 0.1.0"),
        "{}",
        text(&o)
    );
    ok(&cigi(&fx, &["use", "0.1.0"]));
    // a record that lacks a tool its carton is meant to have (by the manifest): refused, E1818
    let state = fx.cigscript().join("installer/state.json");
    let mut j: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&state).unwrap()).unwrap();
    j["cartons"]["0.2.0"]["tools"]
        .as_object_mut()
        .unwrap()
        .remove("cigo");
    std::fs::write(&state, serde_json::to_string_pretty(&j).unwrap()).unwrap();
    let o = cigi(&fx, &["use", "0.2.0"]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(text(&o).contains("E1818"), "{}", text(&o));
    assert!(
        text(&o).contains("ciginstaller install carton --carton 0.2.0"),
        "{}",
        text(&o)
    );
}

/// A shim name the shim binary does not serve never gets a file in bin/ (it would be a dead
/// command first on PATH); the tool's own shim does (SEC-004, PR #37).
#[test]
fn a2r_only_served_shim_names_get_a_file() {
    let mut fx = fixture();
    fx.extra_cigo_shim = Some("cigx".into());
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.17.0")]);
    fx.publish("stable");
    ok(&install(&fx, &["carton", "--carton", "0.1.0"]));
    let bin = fx.cigscript().join("bin");
    assert!(bin.join("cigo").is_file());
    assert!(
        !bin.join("cigx").exists(),
        "a shim for a name the shim refuses"
    );
}

/// Finding 4: a stick of another carton, after that carton was installed whole, is installed
/// and reachable; the second time it is reported as already installed.
#[test]
fn a2r_stick_after_full_install_is_reachable() {
    let mut fx = fixture();
    fx.carton("0.1.0", &[("cig", "1.20.0")]);
    fx.carton("0.2.0", &[("cig", "1.21.0"), ("cigo", "1.17.0")]);
    fx.publish("stable");
    ok(&install(&fx, &["carton", "--carton", "0.1.0"]));
    ok(&install(&fx, &["carton", "--carton", "0.2.0"]));
    let o = shim(&fx, "cigo", &fx.project, &[]);
    assert_eq!(
        o.status.code(),
        Some(127),
        "no cigo is reachable yet: {}",
        text(&o)
    );
    let o = install(&fx, &["stick", "cigo", "--carton", "0.2.0"]);
    ok(&o);
    assert!(!text(&o).contains("already installed"), "{}", text(&o));
    let o = shim(&fx, "cigo", &fx.project, &[]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    assert_eq!(out(&o), "cigo 1.17.0 uses cig 1.20.0");
    let o = install(&fx, &["stick", "cigo", "--carton", "0.2.0"]);
    ok(&o);
    assert!(text(&o).contains("already installed"), "{}", text(&o));
    // a tool the machine carton has at another build: recorded, and the report says what runs
    let o = install(&fx, &["stick", "cig", "--carton", "0.2.0"]);
    ok(&o);
    assert!(text(&o).contains("runs first"), "{}", text(&o));
    let o = install(&fx, &["stick", "cig", "--carton", "0.2.0"]);
    assert!(text(&o).contains("already installed"), "{}", text(&o));
}

/// Finding 11: `never` makes no request at all, and `ask` without a terminal makes none either;
/// `auto` fetches.
#[test]
fn a2r_pin_fetch_policy_before_network() {
    let mut fx = fixture();
    two_cartons(&mut fx);
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
    let proj = pinned(&fx, "pf", "0.2");
    let mirror = server.url();
    let env = |policy: &'static str| [("CIGI_MIRROR", mirror.as_str()), ("CIGI_PIN_FETCH", policy)];
    for policy in ["never", "ask"] {
        server.clear();
        let o = shim(&fx, "cigo", &proj, &env(policy));
        assert_eq!(o.status.code(), Some(2), "{policy}: {}", text(&o));
        assert!(text(&o).contains("E1842"), "{policy}: {}", text(&o));
        assert!(
            text(&o).contains("ciginstaller install carton --carton 0.2"),
            "{}",
            text(&o)
        );
        assert!(
            server.requests().is_empty(),
            "{policy}: {:?}",
            server.paths()
        );
    }
    server.clear();
    let o = shim(&fx, "cigo", &proj, &env("auto"));
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    assert_eq!(last_line(&o), "cigo 1.17.0 uses cig 1.21.0");
    assert!(text(&o).contains("fetching carton"), "{}", text(&o));
    assert!(!server.requests().is_empty());
}

/// Finding 13: the installer lock is not held while the tool runs after a pin fetch. The fake
/// `cig` that cigo calls tries a mutating installer command; it waits 30 s and fails if the
/// resolver still holds the lock.
#[test]
fn a2r_lock_released_while_tool_runs() {
    let mut fx = fixture();
    two_cartons(&mut fx);
    ok(&install(&fx, &["carton", "--carton", "0.1.0"]));
    let proj = pinned(&fx, "lk", "0.2");
    let fakes = fx.tmp.path().join("fakebin");
    std::fs::create_dir_all(&fakes).unwrap();
    let fake = fakes.join("cig");
    let installer = fx.cigscript().join("bin").join("ciginstaller");
    std::fs::write(
        &fake,
        format!(
            "#!/bin/sh\nif \"{}\" --yes install carton --carton 0.2.0 --dry-run >/dev/null 2>&1; then echo free; else echo held; fi\n",
            installer.display()
        ),
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let path = format!(
        "{}:{}:/usr/bin:/bin",
        fakes.display(),
        fx.cigscript().join("bin").display()
    );
    let t = Instant::now();
    let o = shim(
        &fx,
        "cigo",
        &proj,
        &[("CIGI_PIN_FETCH", "auto"), ("PATH", &path)],
    );
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    assert_eq!(last_line(&o), "cigo 1.17.0 uses free", "{}", text(&o));
    assert!(t.elapsed() < Duration::from_secs(20));
}

/// Finding 14: no tool is 127; a refusal is 2; `which` maps only refusals to 2.
#[test]
fn a2r_exit_codes() {
    let mut fx = fixture();
    two_cartons(&mut fx);
    fx.carton("0.3.0", &[("cig", "1.22.0")]);
    fx.publish("stable");
    // the pinned carton has no such tool: the shim found no tool to run
    ok(&install(&fx, &["carton", "--carton", "0.1.0"]));
    ok(&install(&fx, &["stick", "cig", "--carton", "0.3.0"]));
    let none = pinned(&fx, "none", "0.3");
    let o = shim(&fx, "cigo", &none, &[]);
    assert_eq!(o.status.code(), Some(127), "{}", text(&o));
    assert!(text(&o).contains("E1840"), "{}", text(&o));
    let o = which(&fx, "cigo", &none, &[]);
    assert_eq!(o.status.code(), Some(127), "{}", text(&o));
    // a pin that fetching is not allowed to meet is a refusal
    let proj = pinned(&fx, "x", "0.2");
    let o = shim(&fx, "cigo", &proj, &[("CIGI_PIN_FETCH", "never")]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    let o = which(&fx, "cigo", &proj, &[]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(text(&o).contains("E1842"), "{}", text(&o));
    // a fetch the installer refuses on purpose (a yanked carton) stays 2 through the shim
    let mut fx2 = fixture();
    two_cartons(&mut fx2);
    fx2.yank_carton("0.2.0", "bad");
    fx2.publish("stable");
    ok(&install(&fx2, &["carton", "--carton", "0.1.0"]));
    let proj = pinned(&fx2, "y", "0.2.0");
    let o = shim(&fx2, "cigo", &proj, &[("CIGI_PIN_FETCH", "auto")]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(text(&o).contains("E1803"), "{}", text(&o));
}

/// Finding 15: a minor pin passes over a yanked carton (the shim's index and the resolver agree);
/// an exact pin still runs it, with a warning.
#[test]
fn a2r_yanked_carton() {
    let mut fx = fixture();
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.16.0")]);
    fx.carton("0.1.1", &[("cig", "1.20.1"), ("cigo", "1.17.0")]);
    fx.yank_carton("0.1.1", "cigo could skip a file");
    fx.publish("stable");
    ok(&install(&fx, &["carton", "--carton", "0.1.0"]));
    ok(&install(
        &fx,
        &["carton", "--carton", "0.1.1", "--allow-yanked"],
    ));
    let idx = std::fs::read_to_string(fx.cigscript().join("installer/shim-index")).unwrap();
    assert!(idx.contains("carton-minor\t0.1\t0.1.0"), "{idx}");
    let minor = pinned(&fx, "minor", "0.1");
    let o = shim(&fx, "cigo", &minor, &[]);
    assert_eq!(out(&o), "cigo 1.16.0 uses cig 1.20.0", "{}", text(&o));
    let o = which(&fx, "cigo", &minor, &[]);
    assert!(
        text(&o).contains("cigo 1.16.0, from carton 0.1.0"),
        "{}",
        text(&o)
    );
    // a range that reaches the yanked carton passes over it as well
    let ge = pinned(&fx, "ge", ">=0.1.0");
    let o = shim(&fx, "cigo", &ge, &[]);
    assert_eq!(out(&o), "cigo 1.16.0 uses cig 1.20.0", "{}", text(&o));
    // the exact pin runs it, and says so
    let exact = pinned(&fx, "exact", "0.1.1");
    let o = shim(&fx, "cigo", &exact, &[]);
    assert_eq!(out(&o), "cigo 1.17.0 uses cig 1.20.1", "{}", text(&o));
    let o = which(&fx, "cigo", &exact, &[]);
    assert!(
        text(&o).contains("cigo 1.17.0, from carton 0.1.1"),
        "{}",
        text(&o)
    );
}

/// Range pins reach the slow path (`>=`) and pick the newest installed carton that fits.
#[test]
fn a2r_range_pins() {
    let mut fx = fixture();
    two_cartons(&mut fx);
    ok(&install(&fx, &["carton", "--carton", "0.1.0"]));
    ok(&install(&fx, &["carton", "--carton", "0.2.0"]));
    for pin in [">=0.1.0", ">=0.2.0", "0"] {
        let p = pinned(&fx, &format!("r{}", pin.len()), pin);
        let o = shim(&fx, "cigo", &p, &[]);
        assert_eq!(
            out(&o),
            "cigo 1.17.0 uses cig 1.21.0",
            "{pin}: {}",
            text(&o)
        );
        let o = which(&fx, "cigo", &p, &[]);
        assert!(
            text(&o).contains("cigo 1.17.0, from carton 0.2.0 (pinned in .cig-carton)"),
            "{pin}: {}",
            text(&o)
        );
    }
}

/// `CIGI_CARTON` forces an installed carton; one that is not installed is E1816.
#[test]
fn a2r_cigi_carton() {
    let mut fx = fixture();
    two_cartons(&mut fx);
    ok(&install(&fx, &["carton", "--carton", "0.1.0"]));
    ok(&install(&fx, &["carton", "--carton", "0.2.0"]));
    let o = which(&fx, "cigo", &fx.project, &[("CIGI_CARTON", "0.2.0")]);
    ok(&o);
    assert!(
        text(&o).contains("cigo 1.17.0, from carton 0.2.0 (CIGI_CARTON=0.2.0)"),
        "{}",
        text(&o)
    );
    let o = shim(&fx, "cigo", &fx.project, &[("CIGI_CARTON", "0.2.0")]);
    assert_eq!(out(&o), "cigo 1.17.0 uses cig 1.21.0", "{}", text(&o));
    let o = which(&fx, "cigo", &fx.project, &[("CIGI_CARTON", "9.9.9")]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(text(&o).contains("E1816"), "{}", text(&o));
}

/// A pinned carton that is installed but lacks the tool: E1840 with the stick fix, exit 127.
#[test]
fn a2r_pinned_carton_lacks_the_tool() {
    let mut fx = fixture();
    two_cartons(&mut fx);
    ok(&install(&fx, &["carton", "--carton", "0.1.0"]));
    ok(&install(&fx, &["stick", "cig", "--carton", "0.2.0"]));
    let proj = pinned(&fx, "lacks", "0.2");
    let o = shim(&fx, "cigo", &proj, &[]);
    assert_eq!(o.status.code(), Some(127), "{}", text(&o));
    assert!(text(&o).contains("E1840"), "{}", text(&o));
    assert!(
        text(&o).contains("ciginstaller install stick cigo --carton 0.2"),
        "{}",
        text(&o)
    );
}

/// Finding 10: `which` writes `last_used` only when no journal is unfinished.
#[test]
fn a2r_which_leaves_state_alone_while_a_journal_is_open() {
    let mut fx = fixture();
    two_cartons(&mut fx);
    // crash after the flip, before the commit line: state and index are written, the journal stays open
    let o = fx
        .cigi()
        .env("CIGI_ALLOW_SINGLE_SOURCE", "true")
        .env("CIGI_FAULT", "flipped:1")
        .args(["--yes", "install", "carton", "--carton", "0.1.0"])
        .output()
        .unwrap();
    assert!(
        o.status.code().is_none(),
        "the fault did not fire: {}",
        text(&o)
    );
    let before = state_bytes(&fx);
    let o = which(&fx, "cigo", &fx.project, &[]);
    ok(&o);
    assert_eq!(
        state_bytes(&fx),
        before,
        "which wrote state.json under an open journal"
    );
    // once recovered, the same call records the use
    ok(&cigi(
        &fx,
        &["install", "carton", "--carton", "0.1.0", "--dry-run"],
    ));
    let o = which(&fx, "cigo", &fx.project, &[]);
    ok(&o);
    assert_ne!(state_bytes(&fx), before);
    let leftovers: Vec<_> = std::fs::read_dir(fx.cigscript().join("installer"))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

/// On the slow path stdout is the tool's: the fetch line, the progress and every note go to
/// stderr, so `cig ... | jq` reads only what cig printed.
#[test]
fn a2r_slow_path_keeps_stdout_for_the_tool() {
    let mut fx = fixture();
    two_cartons(&mut fx);
    ok(&install(&fx, &["carton", "--carton", "0.1.0"]));
    let p = pinned(&fx, "fetches", "0.2");
    let o = shim(&fx, "cig", &p, &[("CIGI_PIN_FETCH", "auto")]);
    ok(&o);
    let stdout = String::from_utf8_lossy(&o.stdout).to_string();
    assert_eq!(stdout.lines().count(), 1, "stdout: {stdout}");
    assert!(stdout.starts_with("cig 1.21.0"), "stdout: {stdout}");
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert!(stderr.contains("fetching carton"), "stderr: {stderr}");
}
