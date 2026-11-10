// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! Old pins (#6, #25): a script's `# cig >=1.6.2` header, the cigo manifest's `"cig"` floor, and
//! the channel's latest when nothing is installed or pinned. Each is placed by the resolver; the
//! shim only notices that a script has a header and hands it over.
#![cfg(unix)]

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use cigi_testkit::{output_retrying, Fixture};
use common::{cigi, fixture, ok, out, text};

/// A binary run with the fixture's environment plus `extra`, no terminal on stdin. Retries "text
/// file busy" (ETXTBSY), which a freshly written binary can hit under parallel tests.
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

/// 0.1.0 is old (cig 1.5.0, cigo 1.16.0), 0.2.0 is new (cig 1.20.0, cigo 1.17.0).
fn old_and_new(fx: &mut Fixture) {
    fx.carton("0.1.0", &[("cig", "1.5.0"), ("cigo", "1.16.0")]);
    fx.carton("0.2.0", &[("cig", "1.20.0"), ("cigo", "1.17.0")]);
    fx.publish("stable");
}

const NEEDS_NEW: &str = "#!/usr/bin/env cig\n# cig >=1.6.2\nsay \"hi\"\n";

/// #6's Done line: a script with only `# cig >=1.6.2` gets a carton that satisfies it. The machine
/// is on the old carton; the new one is fetched, said once, and run; the machine does not move.
#[test]
fn a5_a_header_gets_a_carton_that_satisfies_it() {
    let mut fx = fixture();
    old_and_new(&mut fx);
    ok(&install(&fx, &["carton", "--carton", "0.1.0"]));
    let proj = project(&fx, "p", &[("s.cig", NEEDS_NEW)]);
    // nothing is fetched while the policy says never: a refusal that names the command
    let before = state(&fx);
    let o = shim(
        &fx,
        "cig",
        &proj,
        &["s.cig"],
        &[("CIGI_PIN_FETCH", "never")],
    );
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    let t = text(&o);
    assert!(t.contains("E1809"), "{t}");
    assert!(
        t.contains("ciginstaller install carton --carton 0.2.0"),
        "the fix is a real command: {t}"
    );
    assert_eq!(state(&fx), before, "a refusal changes nothing");
    // with fetching allowed
    let o = shim(&fx, "cig", &proj, &["s.cig"], &[("CIGI_PIN_FETCH", "auto")]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    let t = text(&o);
    assert!(t.contains("fetching carton 0.2.0 for s.cig"), "{t}");
    assert!(t.contains("cig 1.20.0 args=s.cig carton=0.2.0"), "{t}");
    let st = state(&fx);
    assert!(st["cartons"].get("0.2.0").is_some(), "{st}");
    assert_eq!(
        st["machine_carton"], "0.1.0",
        "the machine stays where it was"
    );
    // the second time nothing is fetched: it is installed now
    let o = shim(
        &fx,
        "cig",
        &proj,
        &["run", "s.cig"],
        &[("CIGI_PIN_FETCH", "never")],
    );
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    assert!(
        text(&o).contains("cig 1.20.0 args=run s.cig"),
        "{}",
        text(&o)
    );
    // and a script without a header, or no script, still runs the machine's cig
    let plain = project(&fx, "plain", &[("t.cig", "say \"hi\"\n")]);
    let o = shim(&fx, "cig", &plain, &["t.cig"], &[]);
    assert!(text(&o).contains("cig 1.5.0 args=t.cig"), "{}", text(&o));
    let o = shim(&fx, "cig", &proj, &["--version"], &[]);
    assert_eq!(out(&o), "cig 1.5.0");
}

/// `which cig --script` says which step decided, for a build that is installed already.
#[test]
fn a5_which_explains_each_header_step() {
    let mut fx = fixture();
    old_and_new(&mut fx);
    ok(&install(&fx, &["carton", "--carton", "0.1.0"]));
    ok(&install(&fx, &["carton", "--carton", "0.2.0"]));
    let proj = project(
        &fx,
        "p",
        &[
            ("s.cig", NEEDS_NEW),
            ("old.cig", "# cig 1.5\n"),
            ("loose.cig", "# cig 1\n"),
        ],
    );
    // the machine's carton does not meet it: the newest installed cig that does
    let o = which(&fx, &proj, &["cig", "--why", "--script", "s.cig"]);
    ok(&o);
    let t = text(&o);
    assert!(
        t.contains("cig 1.20.0, from carton 0.2.0 (satisfies # cig >=1.6.2 in s.cig)"),
        "{t}"
    );
    assert!(
        t.contains("H0. # cig >=1.6.2 in s.cig asks for cig >=1.6.2"),
        "{t}"
    );
    assert!(
        t.contains("H1. cig 1.5.0 (the machine's carton) does not meet it"),
        "{t}"
    );
    assert!(
        t.contains("H2. the newest installed cig that meets it is 1.20.0, in carton 0.2.0"),
        "{t}"
    );
    // the machine's carton meets it: it stays, and the step says so
    let o = which(&fx, &proj, &["cig", "--why", "--script", "old.cig"]);
    ok(&o);
    assert!(
        text(&o).contains("cig 1.5.0, from carton 0.1.0 (the machine's carton)"),
        "{}",
        text(&o)
    );
    assert!(text(&o).contains("H1. cig 1.5.0 meets it"), "{}", text(&o));
    let o = which(&fx, &proj, &["cig", "--why", "--script", "loose.cig"]);
    assert!(text(&o).contains("H1. cig 1.5.0 meets it"), "{}", text(&o));
    // --json carries the script
    let o = which(&fx, &proj, &["cig", "--json", "--script", "s.cig"]);
    ok(&o);
    let j: serde_json::Value = serde_json::from_str(&out(&o)).unwrap();
    let d = j.get("data").unwrap_or(&j);
    assert_eq!(d["script"]["header"], ">=1.6.2", "{j}");
    assert!(
        d["script"]["file"].as_str().unwrap().ends_with("/s.cig"),
        "{j}"
    );
    // `which` never fetches, and says what a run would do
    let mut fx2 = fixture();
    old_and_new(&mut fx2);
    ok(&install(&fx2, &["carton", "--carton", "0.1.0"]));
    let p2 = project(&fx2, "p", &[("s.cig", NEEDS_NEW)]);
    let o = which(&fx2, &p2, &["cig", "--why", "--script", "s.cig"]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    let t = text(&o);
    assert!(t.contains("E1809"), "{t}");
    assert!(t.contains("H2. no installed cig meets it"), "{t}");
    // no manifest cached on this fixture's channel yet? the fix is still a real command
    assert!(
        t.contains("ciginstaller carton list") || t.contains("--carton 0.2.0"),
        "{t}"
    );
}

/// A header wins over `.cig-version`, as cig reads them; the other forms count too.
#[test]
fn a5_header_forms_and_the_version_file() {
    let mut fx = fixture();
    old_and_new(&mut fx);
    ok(&install(&fx, &["carton", "--carton", "0.1.0"]));
    ok(&install(&fx, &["carton", "--carton", "0.2.0"]));
    let proj = project(
        &fx,
        "p",
        &[
            (".cig-version", "1.5\n"),
            ("colon.cig", "#!/usr/bin/env cig\n# cig: >=1.6.2\n"),
            ("v.cig", "\n# a tool\n#cig v1.20.0\n"),
            ("usage.cig", "# cig run s.cig -- x\nsay \"hi\"\n"),
            ("late.cig", "say \"hi\"\n# cig >=1.6.2\n"),
        ],
    );
    let want = |script: &str, build: &str| {
        let o = shim(
            &fx,
            "cig",
            &proj,
            &["run", script],
            &[("CIGI_PIN_FETCH", "never")],
        );
        assert_eq!(o.status.code(), Some(0), "{script}: {}", text(&o));
        assert!(
            text(&o).contains(&format!("cig {build} args=run {script}")),
            "{script}: {}",
            text(&o)
        );
    };
    want("colon.cig", "1.20.0");
    want("v.cig", "1.20.0");
    // not pins: the project's `.cig-version` (1.5) decides, which the machine's carton meets
    want("usage.cig", "1.5.0");
    want("late.cig", "1.5.0");
    let o = which(&fx, &proj, &["cig", "--why", "--script", "colon.cig"]);
    assert!(
        text(&o).contains(".cig-version 1.5 is set aside: the script's header wins over it"),
        "{}",
        text(&o)
    );
}

/// The shim looks at a script only when the first argument that is not a flag is a readable
/// file; everything else stays on the fast path, which does not need the installer at all.
#[test]
fn a5_the_shim_peeks_only_at_a_named_script() {
    let mut fx = fixture();
    old_and_new(&mut fx);
    ok(&install(&fx, &["carton", "--carton", "0.1.0"]));
    let proj = project(&fx, "p", &[("s.cig", NEEDS_NEW), ("t.cig", "say \"hi\"\n")]);
    std::fs::create_dir_all(proj.join("dir.cig")).unwrap();
    // the slow path is the installer: without it, only a header can fail
    std::fs::remove_file(fx.cigscript().join("bin/ciginstaller")).unwrap();
    for args in [
        &["--version"][..],
        &["t.cig"][..],
        &["run", "t.cig"][..],
        &["run", "missing.cig"][..],
        &["dir.cig"][..],
        &["--flag", "versions", "list"][..],
    ] {
        let o = shim(&fx, "cig", &proj, args, &[]);
        assert_eq!(o.status.code(), Some(0), "{args:?}: {}", text(&o));
        assert!(text(&o).contains("cig 1.5.0"), "{args:?}: {}", text(&o));
    }
    for args in [&["s.cig"][..], &["--deny-warnings", "check", "s.cig"][..]] {
        let o = shim(&fx, "cig", &proj, args, &[]);
        assert_eq!(o.status.code(), Some(127), "{args:?}: {}", text(&o));
        assert!(text(&o).contains("E1840"), "{args:?}: {}", text(&o));
    }
    // CIGI_CARTON is the user's word: no look at the script at all
    let o = shim(&fx, "cig", &proj, &["s.cig"], &[("CIGI_CARTON", "0.1.0")]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    assert!(text(&o).contains("cig 1.5.0 args=s.cig"), "{}", text(&o));
    // only cig reads headers
    let o = shim(&fx, "cigo", &proj, &["s.cig"], &[]);
    assert!(text(&o).contains("cigo 1.16.0"), "{}", text(&o));
}

/// cig-orchestrator names no cigo version a project needs; its manifest names the cig the project
/// is written for (`"cig"` in cig-tasks.json), and that floor is honored for every tool.
#[test]
fn a5_the_cigo_manifests_cig_floor() {
    let mut fx = fixture();
    old_and_new(&mut fx);
    ok(&install(&fx, &["carton", "--carton", "0.1.0"]));
    ok(&install(&fx, &["carton", "--carton", "0.2.0"]));
    let manifest =
        r#"{"project":"p","cig":">=1.6.2","components":{"cig":{"path":"."}},"tasks":{}}"#;
    let proj = project(&fx, "p", &[("cig-tasks.json", manifest)]);
    let sub = proj.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    // a project that names a floor gets the carton that meets it, for cigo as for cig
    let o = shim(&fx, "cigo", &sub, &[], &[("CIGI_PIN_FETCH", "never")]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    assert!(text(&o).starts_with("cigo 1.17.0"), "{}", text(&o));
    let o = shim(
        &fx,
        "cig",
        &sub,
        &["--version"],
        &[("CIGI_PIN_FETCH", "never")],
    );
    assert_eq!(out(&o), "cig 1.20.0");
    let o = which(&fx, &sub, &["cigo", "--why"]);
    ok(&o);
    let t = text(&o);
    assert!(
        t.contains(r#"cigo 1.17.0, from carton 0.2.0 (satisfies cig-tasks.json "cig" >=1.6.2)"#),
        "{t}"
    );
    assert!(
        t.contains(r#"3. legacy pin files: cig-tasks.json "cig" >=1.6.2"#),
        "{t}"
    );
    // a manifest with no top-level "cig" (a component called cig is not one) pins nothing
    let plain = project(
        &fx,
        "plain",
        &[(
            "cig-tasks.json",
            r#"{"components":{"cig":"1.20.0"},"tasks":{}}"#,
        )],
    );
    let o = shim(&fx, "cigo", &plain, &[], &[]);
    assert!(text(&o).starts_with("cigo 1.16.0"), "{}", text(&o));
    // a floor nothing installed meets is fetched like any pin: one stick, the rest from the base
    let mut fx2 = fixture();
    old_and_new(&mut fx2);
    ok(&install(&fx2, &["carton", "--carton", "0.1.0"]));
    let p2 = project(&fx2, "p", &[("cig-tasks.json", manifest)]);
    let o = shim(
        &fx2,
        "cig",
        &p2,
        &["--version"],
        &[("CIGI_PIN_FETCH", "never")],
    );
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(
        text(&o).contains("E1842") || text(&o).contains("E1801"),
        "{}",
        text(&o)
    );
    let o = shim(
        &fx2,
        "cig",
        &p2,
        &["--version"],
        &[("CIGI_PIN_FETCH", "auto")],
    );
    assert_eq!(out(&o), "cig 1.20.0", "{}", text(&o));
    // a floor that is not a constraint is the pin file's own error
    let bad = project(
        &fx2,
        "bad",
        &[("cig-tasks.json", r#"{"cig":"newer please"}"#)],
    );
    let o = shim(&fx2, "cigo", &bad, &[], &[]);
    assert!(text(&o).contains("E1841"), "{}", text(&o));
}

/// #6's last step: nothing pinned and nothing installed runs the channel's latest.
#[test]
fn a5_nothing_installed_runs_the_channels_latest() {
    let mut fx = fixture();
    old_and_new(&mut fx);
    // a home with shims and an installer, and no carton: install one, then take it away again
    ok(&install(&fx, &["carton", "--carton", "0.1.0"]));
    let path = fx.cigscript().join("installer/state.json");
    let mut j = state(&fx);
    j["cartons"] = serde_json::json!({});
    j["machine_carton"] = serde_json::Value::Null;
    j["sticks"] = serde_json::json!([]);
    std::fs::write(&path, serde_json::to_string_pretty(&j).unwrap()).unwrap();
    // the shim's index of an empty machine names nothing, so every run goes to the resolver
    std::fs::write(
        fx.cigscript().join("installer/shim-index"),
        "cigi-index 1\n",
    )
    .unwrap();
    assert!(
        state(&fx)["cartons"].as_object().unwrap().is_empty(),
        "{}",
        state(&fx)
    );
    let proj = project(&fx, "p", &[]);
    // `which` never fetches: it says what a run would do
    let o = which(&fx, &proj, &["cig", "--why"]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    let t = text(&o);
    assert!(t.contains("E1819"), "{t}");
    assert!(
        t.contains("6. nothing is installed or pinned here, so the channel's latest"),
        "{t}"
    );
    assert!(t.contains("ciginstaller install carton"), "{t}");
    // fetching off: a refusal, nothing installed
    let o = shim(
        &fx,
        "cig",
        &proj,
        &["--version"],
        &[("CIGI_PIN_FETCH", "never")],
    );
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(text(&o).contains("E1819"), "{}", text(&o));
    assert!(state(&fx)["cartons"].as_object().unwrap().is_empty());
    // fetching on: the channel's latest, and the machine is on it from then
    let o = shim(
        &fx,
        "cig",
        &proj,
        &["--version"],
        &[("CIGI_PIN_FETCH", "auto")],
    );
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    assert!(
        text(&o).contains("fetching carton 0.2.0, the channel's latest"),
        "{}",
        text(&o)
    );
    assert!(text(&o).contains("cig 1.20.0"), "{}", text(&o));
    assert_eq!(state(&fx)["machine_carton"], "0.2.0");
    let o = which(&fx, &proj, &["cig", "--why"]);
    ok(&o);
    assert!(
        text(&o).contains("cig 1.20.0, from carton 0.2.0 (the machine's carton)"),
        "{}",
        text(&o)
    );
}

/// A machine that has a carton, but not the tool, is told how to add the tool; it is not handed a
/// second carton.
#[test]
fn a5_a_machine_with_a_carton_does_not_fall_to_the_channel() {
    let mut fx = fixture();
    fx.carton("0.1.0", &[("cig", "1.20.0")]);
    fx.carton("0.2.0", &[("cig", "1.21.0"), ("cigo", "1.17.0")]);
    fx.publish("stable");
    ok(&install(&fx, &["carton", "--carton", "0.1.0"]));
    let proj = project(&fx, "p", &[]);
    let o = run_in(
        &fx,
        &cigi_testkit::binary_path("ciginstaller"),
        &proj,
        &[
            "__resolve",
            "--tool",
            "cigo",
            "--cwd",
            proj.to_str().unwrap(),
            "--",
        ],
        &[("CIGI_PIN_FETCH", "auto")],
    );
    assert_eq!(o.status.code(), Some(127), "{}", text(&o));
    assert!(text(&o).contains("E1840"), "{}", text(&o));
    assert!(state(&fx)["cartons"].get("0.2.0").is_none());
}
