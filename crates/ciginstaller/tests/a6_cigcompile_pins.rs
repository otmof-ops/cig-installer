// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! cig-compile #15: a project pinned to a CigarScript version (`.cigar-version`, or `cigar = "1.0"`
//! in `cigar.toml`) is built by the cigcompile of the carton that holds that cigar, and stays
//! there after a newer carton is installed. The installer picks; cigcompile never dispatches.
#![cfg(unix)]

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use cigi_testkit::{output_retrying, Fixture};
use common::{cigi, fixture, ok, out, text};

fn run_in(fx: &Fixture, exe: &Path, cwd: &Path, args: &[&str], extra: &[(&str, &str)]) -> Output {
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
    output_retrying(&mut c).expect("run")
}

fn shim(fx: &Fixture, tool: &str, cwd: &Path, args: &[&str], extra: &[(&str, &str)]) -> Output {
    run_in(fx, &fx.cigscript().join("bin").join(tool), cwd, args, extra)
}

fn which(fx: &Fixture, cwd: &Path, args: &[&str]) -> Output {
    let mut v = vec!["which"];
    v.extend_from_slice(args);
    run_in(fx, &cigi_testkit::binary_path("ciginstaller"), cwd, &v, &[])
}

fn project(fx: &Fixture, name: &str, files: &[(&str, &str)]) -> PathBuf {
    let p = fx.tmp.path().join(name);
    std::fs::create_dir_all(&p).unwrap();
    for (f, c) in files {
        std::fs::write(p.join(f), c).unwrap();
    }
    p
}

fn install(fx: &Fixture, args: &[&str]) -> Output {
    let mut v = vec!["install"];
    v.extend_from_slice(args);
    cigi(fx, &v)
}

fn state(fx: &Fixture) -> serde_json::Value {
    let raw = std::fs::read_to_string(fx.cigscript().join("installer/state.json")).unwrap();
    serde_json::from_str(&raw).unwrap()
}

/// 0.1.0 holds cigar 1.0.0 and cigcompile 1.0.0; 0.2.0 holds cigar 1.1.0 and cigcompile 1.1.0.
fn two_cartons(fx: &mut Fixture) {
    fx.carton(
        "0.1.0",
        &[
            ("cig", "1.5.0"),
            ("cigar", "1.0.0"),
            ("cigcompile", "1.0.0"),
        ],
    );
    fx.carton(
        "0.2.0",
        &[
            ("cig", "1.20.0"),
            ("cigar", "1.1.0"),
            ("cigcompile", "1.1.0"),
        ],
    );
    fx.publish("stable");
}

/// Both cartons installed; the machine is on the new one.
fn both_installed() -> Fixture {
    let mut fx = fixture();
    two_cartons(&mut fx);
    ok(&install(&fx, &["carton", "--carton", "0.1.0"]));
    ok(&install(&fx, &["carton", "--carton", "0.2.0"]));
    ok(&cigi(&fx, &["use", "0.2.0"]));
    assert_eq!(state(&fx)["machine_carton"], "0.2.0");
    fx
}

/// #15's Done line: a project pinned to 1.0 builds with 1.0 after 1.1 is installed, whichever
/// file holds the pin; a project that pins nothing gets the machine's 1.1.
#[test]
fn a6_a_project_pinned_to_cigar_1_0_builds_with_cigcompile_1_0() {
    let fx = both_installed();
    let files: [(&str, &str, &str); 2] = [
        ("legacy", ".cigar-version", "1.0\n"),
        ("toml", "cigar.toml", "name = \"app\"\ncigar = \"1.0\"\n"),
    ];
    for (dir, file, body) in files {
        let proj = project(&fx, dir, &[(file, body)]);
        let sub = proj.join("src");
        std::fs::create_dir_all(&sub).unwrap();
        let before = state(&fx);
        for cwd in [&proj, &sub] {
            let o = shim(
                &fx,
                "cigcompile",
                cwd,
                &["--version"],
                &[("CIGI_PIN_FETCH", "never")],
            );
            assert_eq!(o.status.code(), Some(0), "{file}: {}", text(&o));
            assert_eq!(out(&o), "cigcompile 1.0.0", "{file}");
            let o = shim(
                &fx,
                "cigcompile",
                cwd,
                &["build", "app.cig"],
                &[("CIGI_PIN_FETCH", "never")],
            );
            assert!(
                text(&o).contains("cigcompile 1.0.0 args=build app.cig carton=0.1.0"),
                "{file}: {}",
                text(&o)
            );
        }
        // the cigar of the same carton goes with it
        let o = shim(
            &fx,
            "cigar",
            &proj,
            &["--version"],
            &[("CIGI_PIN_FETCH", "never")],
        );
        assert_eq!(out(&o), "cigar 1.0.0", "{file}");
        // the pin's reason is named, and `which` agrees with the shim
        let o = which(&fx, &sub, &["cigcompile", "--why"]);
        ok(&o);
        let t = text(&o);
        assert!(
            t.contains("CigCompile 1.0.0, from carton 0.1.0"),
            "{file}: {t}"
        );
        let label = if file == "cigar.toml" {
            "satisfies cigar.toml \"cigar\" 1.0)".to_string()
        } else {
            "satisfies .cigar-version 1.0)".to_string()
        };
        assert!(t.contains(&label), "{file}: {t}");
        // nothing moved
        let after = state(&fx);
        assert_eq!(
            after["history"], before["history"],
            "{file}: nothing installed"
        );
        assert_eq!(after["machine_carton"], before["machine_carton"], "{file}");
        assert_eq!(after["sticks"], before["sticks"], "{file}");
    }
    // an unpinned folder is on the machine's carton: 1.1
    let plain = project(&fx, "plain", &[]);
    let o = shim(&fx, "cigcompile", &plain, &["--version"], &[]);
    assert_eq!(out(&o), "cigcompile 1.1.0", "{}", text(&o));
    // a pin that the new build meets takes the newest that does
    let proj = project(&fx, "new", &[("cigar.toml", "cigar = \"1.1\"\n")]);
    let o = shim(&fx, "cigcompile", &proj, &["--version"], &[]);
    assert_eq!(out(&o), "cigcompile 1.1.0", "{}", text(&o));
    // the machine's carton is where it was
    assert_eq!(state(&fx)["machine_carton"], "0.2.0");
}

/// A `carton` key in `cigar.toml` is a carton pin, and wins over its `cigar` key (step 2 of the
/// resolver); a table-level `cigar` key is not the top-level one and pins nothing.
#[test]
fn a6_the_carton_key_wins_and_a_table_key_pins_nothing() {
    let fx = both_installed();
    let proj = project(
        &fx,
        "both",
        &[("cigar.toml", "carton = \"0.2\"\ncigar = \"1.0\"\n")],
    );
    let o = shim(&fx, "cigcompile", &proj, &["--version"], &[]);
    assert_eq!(out(&o), "cigcompile 1.1.0", "{}", text(&o));
    let table = project(
        &fx,
        "table",
        &[("cigar.toml", "[package]\ncigar = \"1.0\"\n")],
    );
    let o = shim(&fx, "cigcompile", &table, &["--version"], &[]);
    assert_eq!(out(&o), "cigcompile 1.1.0", "{}", text(&o));
    // a `cigar` value that is not a constraint is the pin file's own error, never a guess
    let bad = project(&fx, "bad", &[("cigar.toml", "cigar = \"newer please\"\n")]);
    let o = shim(&fx, "cigcompile", &bad, &["--version"], &[]);
    assert!(text(&o).contains("E1841"), "{}", text(&o));
}

/// A pin nothing installed meets is fetched like any pin when the policy allows, and refused with
/// a command to run when it does not. The machine does not move either way.
#[test]
fn a6_a_cigar_pin_nothing_installed_meets() {
    let mut fx = fixture();
    two_cartons(&mut fx);
    ok(&install(&fx, &["carton", "--carton", "0.2.0"]));
    assert_eq!(state(&fx)["machine_carton"], "0.2.0");
    let proj = project(&fx, "p", &[("cigar.toml", "cigar = \"1.0\"\n")]);
    let before = state(&fx);
    let o = shim(
        &fx,
        "cigcompile",
        &proj,
        &["--version"],
        &[("CIGI_PIN_FETCH", "never")],
    );
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    let t = text(&o);
    assert!(t.contains("E1801") || t.contains("E1842"), "{t}");
    assert_eq!(
        state(&fx)["history"],
        before["history"],
        "a refusal installs nothing"
    );
    let o = shim(
        &fx,
        "cigcompile",
        &proj,
        &["--version"],
        &[("CIGI_PIN_FETCH", "auto")],
    );
    assert_eq!(out(&o), "cigcompile 1.0.0", "{}", text(&o));
    let st = state(&fx);
    assert_eq!(st["machine_carton"], "0.2.0", "the machine stays put");
    assert_eq!(
        st["sticks"],
        serde_json::json!([]),
        "a whole carton, not a stick of cigar"
    );
    assert!(st["cartons"].get("0.1.0").is_some(), "{st}");
}
