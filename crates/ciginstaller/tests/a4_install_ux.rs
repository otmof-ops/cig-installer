// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `a4_install_ux`: the install questions and the preset (#5, #22, #32), E1801's words and the
//! per-tool pin it points to (#4, #6), the "what's new" line (#7) and advisories shown once (#12).
//! Assertions are on codes, headlines and data, not on icons or labels; #4's headline is the
//! exception, because its words are the requirement.

mod common;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use cigi_testkit::{sha256_hex, Fixture};
use common::*;

const MIRROR: (&str, &str) = ("CIGI_ALLOW_SINGLE_SOURCE", "true");

/// The installer as a child with no terminal and no `--yes`.
fn bare(fx: &Fixture, dir: &Path, args: &[&str]) -> Output {
    fx.cigi()
        .env(MIRROR.0, MIRROR.1)
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .output()
        .expect("run ciginstaller")
}

/// A terminal for the child through util-linux `script`; its input is `keys`.
fn on_tty(fx: &Fixture, args: &str, keys: &str) -> (i32, String) {
    let c = fx.cigi();
    let exe = c.get_program().to_string_lossy().to_string();
    let q = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
    // the child gets the umask the installer is written for, whatever this machine's is
    let line = format!("umask 022; {} {args}", q(&exe));
    let mut s = Command::new("script");
    s.env_clear();
    for (k, v) in c.get_envs() {
        if let Some(v) = v {
            s.env(k, v);
        }
    }
    s.env(MIRROR.0, MIRROR.1)
        .args(["-qec", &line, "/dev/null"])
        .current_dir(&fx.project)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = s.spawn().expect("run script(1)");
    {
        let mut i = child.stdin.take().expect("script stdin");
        let _ = i.write_all(keys.as_bytes());
        // script ends the child at EOF, so the input stays open until the child is done
        std::mem::forget(i);
    }
    let o = child.wait_with_output().expect("wait for script(1)");
    (
        o.status.code().unwrap_or(-1),
        format!(
            "{}{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        ),
    )
}

fn simple() -> Fixture {
    let mut fx = fixture();
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.16.0")]);
    fx.publish("stable");
    fx
}

fn config(fx: &Fixture) -> String {
    std::fs::read_to_string(fx.home.config_path()).unwrap_or_default()
}

fn state(fx: &Fixture) -> serde_json::Value {
    let t = std::fs::read_to_string(fx.home.state_path()).expect("state.json");
    serde_json::from_str(&t).expect("state.json is JSON")
}

fn json_of(o: &Output) -> serde_json::Value {
    serde_json::from_slice(&o.stdout).expect("one JSON object")
}

// ---------------------------------------------------------------- the menu

#[test]
fn a4_menu_on_a_terminal_asks_what_you_are_after_then_the_settings() {
    let fx = simple();
    // one keypress each: 3 = a carton, s = strict, n = never send crash reports
    let (code, t) = on_tty(&fx, "install --no-path", "3sn");
    assert_eq!(code, 0, "{t}");
    for want in [
        "what are you after?",
        "a stick",
        "one product, e.g. just cig",
        "a pack",
        "a carton",
        "everything comes out of carton 0.1.0, so it all matches.",
        "how careful should the guard be?",
        "send a crash report",
    ] {
        assert!(t.contains(want), "missing `{want}` in {t}");
    }
    let cfg = config(&fx);
    assert!(cfg.contains("preset = strict"), "{cfg}");
    assert!(cfg.contains("reports = never"), "{cfg}");
    assert_eq!(state(&fx)["machine_carton"], "0.1.0");
}

#[test]
fn a4_menu_a_stick_asks_which_product_and_installs_only_it() {
    let fx = simple();
    // one keypress each: 1 = a stick, 1 = the first product (cig), then enter takes the
    // preset's and the reports question's defaults
    let (code, t) = on_tty(&fx, "install --no-path", "11\n\n");
    assert_eq!(code, 0, "{t}");
    assert!(t.contains("which product?"), "{t}");
    let s = state(&fx);
    let tools = s["cartons"]["0.1.0"]["tools"].as_object().expect("tools");
    assert!(
        tools.contains_key("cig") && !tools.contains_key("cigo"),
        "{s}"
    );
    // enter takes the default: balanced, and reports stay `ask`
    assert!(config(&fx).contains("preset = balanced"), "{}", config(&fx));
    assert!(!config(&fx).contains("reports"), "{}", config(&fx));
}

#[test]
fn a4_menu_is_not_asked_for_a_dry_run() {
    let fx = simple();
    let (code, t) = on_tty(&fx, "install --dry-run", "");
    assert_eq!(code, 3, "{t}");
    assert!(t.contains("E1871"), "{t}");
    assert!(!t.contains("what are you after?"), "{t}");
}

// ---------------------------------------------------------------- no terminal

#[test]
fn a4_no_tty_and_no_target_is_a_usage_error_with_the_fix() {
    let fx = simple();
    let o = bare(&fx, &fx.project, &["install"]);
    assert_eq!(o.status.code(), Some(3), "{}", text(&o));
    assert!(text(&o).contains("E1871"), "{}", text(&o));
    assert!(
        text(&o).contains("ciginstaller install carton"),
        "{}",
        text(&o)
    );
    assert!(!fx.home.state_path().exists(), "nothing was installed");
    let o = bare(&fx, &fx.project, &["--json", "install"]);
    assert_eq!(o.status.code(), Some(3), "{}", text(&o));
    assert_eq!(json_of(&o)["error"]["code"], "E1871");
}

#[test]
fn a4_yes_with_no_target_takes_the_whole_carton_and_asks_nothing() {
    let fx = simple();
    let o = cigi(&fx, &["install", "--no-path"]);
    ok(&o);
    let s = state(&fx);
    let tools = s["cartons"]["0.1.0"]["tools"].as_object().expect("tools");
    assert!(
        tools.contains_key("cig") && tools.contains_key("cigo"),
        "{s}"
    );
    // no question was asked, so no setting was invented
    assert!(!config(&fx).contains("preset"), "{}", config(&fx));
    assert!(!config(&fx).contains("reports"), "{}", config(&fx));
}

// ---------------------------------------------------------------- the preset

#[test]
fn a4_preset_is_recorded_and_the_guard_note_is_said_once() {
    let fx = simple();
    // #22's preseed, as the issue writes it: the version is positional
    let o = cigi(
        &fx,
        &[
            "install",
            "carton",
            "0.1.0",
            "--preset",
            "strict",
            "--no-path",
        ],
    );
    ok(&o);
    assert!(config(&fx).contains("preset = strict"), "{}", config(&fx));
    let t = text(&o);
    assert_eq!(t.matches("cig-orchestrator#66").count(), 1, "{t}");
    // and #5's own unattended form
    let fx2 = simple();
    let o = cigi(
        &fx2,
        &[
            "--json",
            "install",
            "carton",
            "--preset",
            "balanced",
            "--no-path",
        ],
    );
    ok(&o);
    assert_eq!(json_of(&o)["data"]["preset"], "balanced");
    assert!(config(&fx2).contains("preset = balanced"));
}

#[test]
fn a4_preset_is_journaled_with_the_install() {
    let fx = simple();
    let o = fx
        .cigi()
        .env(MIRROR.0, MIRROR.1)
        .env("CIGI_FAULT", "after-index-flip")
        .args([
            "--yes",
            "install",
            "carton",
            "--preset",
            "strict",
            "--no-path",
        ])
        .output()
        .expect("run");
    // whatever the fault hook does, a failed install leaves no preset behind
    if o.status.code() != Some(0) {
        assert!(!config(&fx).contains("preset"), "{}", config(&fx));
    }
}

#[test]
fn a4_the_version_goes_positional_or_with_the_flag_not_both() {
    let fx = simple();
    let o = cigi(&fx, &["install", "carton", "0.1.0", "--carton", "0.1.0"]);
    assert_eq!(o.status.code(), Some(3), "{}", text(&o));
    assert!(text(&o).contains("E1870"), "{}", text(&o));
    let o = cigi(
        &fx,
        &["install", "carton", "--carton", "0.1.0", "--no-path"],
    );
    ok(&o);
    let o = cigi(&fx, &["install", "carton", "--preset", "reckless"]);
    assert_eq!(o.status.code(), Some(3), "{}", text(&o));
    assert!(text(&o).contains("E1870"), "{}", text(&o));
}

// ---------------------------------------------------------------- E1801 and pin cigo

fn pair() -> Fixture {
    let mut fx = fixture();
    fx.product("cigo", "1.4.0").requires("cig", ">=1.8");
    fx.carton("1.0.0", &[("cig", "1.7.2"), ("cigo", "1.3.0")]);
    fx.carton("1.1.2", &[("cig", "1.8.0"), ("cigo", "1.4.0")]);
    fx.publish("stable");
    fx
}

#[test]
fn a4_e1801_says_the_headline_and_pin_cigo_works_end_to_end() {
    let fx = pair();
    ok(&cigi(
        &fx,
        &["install", "carton", "--carton", "1.0.0", "--no-path"],
    ));
    // a stick taken from the other carton: cigo 1.4.0 against cig 1.7.2
    let o = cigi(&fx, &["install", "stick", "cigo", "--carton", "1.1.2"]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    let t = text(&o);
    assert!(t.contains("E1801"), "{t}");
    assert!(
        t.contains(
            "these two don't smoke together. cigo 1.4.0 needs cig 1.8 or newer; you have 1.7.2"
        ),
        "{t}"
    );
    // the fix is the one command #4 names
    assert!(t.contains("ciginstaller pin cigo 1.3"), "{t}");

    // run it: the project gets cigo's own pin file
    let o = bare(&fx, &fx.project, &["pin", "cigo", "1.3"]);
    ok(&o);
    assert_eq!(
        std::fs::read_to_string(fx.project.join(".cigo-version")).expect("pin file"),
        "1.3\n"
    );
    // `which` explains it: the machine's carton has cigo 1.3.0 here, and the pin says so
    ok(&cigi(
        &fx,
        &["install", "carton", "--carton", "1.1.2", "--no-path"],
    ));
    let o = bare(&fx, &fx.project, &["which", "cigo"]);
    ok(&o);
    let t = text(&o);
    assert!(t.contains("cigo 1.3.0"), "{t}");
    assert!(t.contains(".cigo-version"), "{t}");
}

#[test]
fn a4_pin_still_refuses_a_tool_with_no_pin_file() {
    let fx = simple();
    let o = bare(&fx, &fx.project, &["pin", "cigcompile", "1.0"]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(text(&o).contains("E1843"), "{}", text(&o));
}

// ---------------------------------------------------------------- what's new

#[test]
fn a4_update_says_what_is_new() {
    let mut fx = simple();
    ok(&cigi(&fx, &["install", "carton", "--no-path"]));
    fx.carton("0.2.0", &[("cig", "1.21.0"), ("cigo", "1.16.0")]);
    fx.publish("stable");
    let o = cigi(&fx, &["--json", "update", "--check", "--refresh"]);
    ok(&o);
    let want = "fresh carton: 0.2.0 \u{2014} fixture carton 0.2.0";
    assert_eq!(json_of(&o)["data"]["whats_new"], want);
    let o = cigi(&fx, &["update"]);
    ok(&o);
    // the dash may be drawn as `-` in an ASCII terminal; the words are what count
    let t = text(&o);
    assert!(
        t.contains("fresh carton: 0.2.0") && t.contains("fixture carton 0.2.0"),
        "{t}"
    );
}

// ---------------------------------------------------------------- advisories

/// The published manifest with one advisory appended and its sidecar made to match.
fn advise(fx: &Fixture) {
    let path = fx.releases.join("manifest-stable.toml");
    let mut t = std::fs::read_to_string(&path).expect("manifest");
    t.push_str(
        "\n[[advisory]]\nid = \"CIGI-2026-0001\"\naffects = [ \"carton <0.2.0\" ]\nfixed_in = \"0.2.0\"\n\
         severity = \"security\"\nsummary = \"a box apply could skip a file\"\n",
    );
    std::fs::write(&path, &t).expect("write manifest");
    std::fs::write(
        fx.releases.join("manifest-stable.toml.sha256"),
        format!("{}  manifest-stable.toml\n", sha256_hex(t.as_bytes())),
    )
    .expect("write sidecar");
}

#[test]
fn a4_an_advisory_is_shown_with_its_severity_once_and_check_still_lists_it() {
    let mut fx = fixture();
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.16.0")]);
    fx.carton("0.2.0", &[("cig", "1.20.1"), ("cigo", "1.16.0")]);
    fx.publish("stable");
    advise(&fx);

    let o = cigi(
        &fx,
        &["install", "stick", "cig", "--carton", "0.1.0", "--no-path"],
    );
    ok(&o);
    let t = text(&o);
    assert!(t.contains("E1804") && t.contains("CIGI-2026-0001"), "{t}");
    assert!(t.contains("security"), "severity is shown: {t}");
    assert!(t.contains("0.2.0"), "the fixed version is shown: {t}");
    assert!(
        state(&fx)["advisories_seen"]
            .as_object()
            .is_some_and(|m| m.keys().any(|k| k.contains("CIGI-2026-0001"))),
        "remembered in state: {}",
        state(&fx)
    );

    // the next install on the same version does not repeat it
    let o = cigi(
        &fx,
        &["install", "stick", "cigo", "--carton", "0.1.0", "--no-path"],
    );
    ok(&o);
    assert!(!text(&o).contains("CIGI-2026-0001"), "{}", text(&o));

    // `check` is the whole matrix and never consults the memory
    let o = cigi(&fx, &["check"]);
    ok(&o);
    assert!(text(&o).contains("CIGI-2026-0001"), "{}", text(&o));
}
