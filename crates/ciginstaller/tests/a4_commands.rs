// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `a4_commands`: the commands the installer's own next-step lines point at: `config`,
//! `history`, `shims`, `status` (and a bare `ciginstaller`) and `which` with no tool.
//!
//! The tests assert on exit codes, error codes, headlines and `--json` data, never on glyphs.
#![cfg(unix)]

mod common;
use std::os::unix::fs::MetadataExt;
use std::process::Output;

use cigi_testkit::Fixture;
use common::*;
use serde_json::Value;

fn json(o: &Output) -> Value {
    serde_json::from_slice(&o.stdout).unwrap_or_else(|e| panic!("{e}: {}", text(o)))
}

fn data(fx: &Fixture, args: &[&str]) -> Value {
    let mut a = vec!["--json"];
    a.extend_from_slice(args);
    let o = cigi(fx, &a);
    ok(&o);
    json(&o)["data"].clone()
}

fn config_file(fx: &Fixture) -> String {
    std::fs::read_to_string(fx.cigscript().join("installer/config")).unwrap_or_default()
}

fn index_path(fx: &Fixture) -> std::path::PathBuf {
    fx.cigscript().join("installer/shim-index")
}

/// A machine on carton 0.1.0 (cig 1.20.0, cigo 1.17.0).
fn installed() -> Fixture {
    let mut fx = fixture();
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.17.0")]);
    fx.publish("stable");
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    fx
}

fn ino(p: &std::path::Path) -> u64 {
    std::fs::metadata(p).unwrap().ino()
}

// ---------------------------------------------------------------- config

#[test]
fn a4_config_lists_every_setting_with_its_source() {
    let fx = fixture();
    let d = data(&fx, &["config"]);
    let settings = d["settings"].as_array().unwrap();
    let keys: Vec<&str> = settings
        .iter()
        .map(|s| s["key"].as_str().unwrap())
        .collect();
    for k in [
        "channel",
        "auto_update",
        "update_check",
        "pin_fetch",
        "trust",
        "allow_single_source",
        "keep_days",
        "mirror",
        "reports",
        "preset",
        "theme",
    ] {
        assert!(keys.contains(&k), "{k} missing from {keys:?}");
    }
    let channel = settings.iter().find(|s| s["key"] == "channel").unwrap();
    assert_eq!(channel["value"], "stable");
    assert_eq!(channel["source"], "default");
    assert_eq!(d["config"]["keep_days"], "90");
    // `config list` is the same screen
    assert_eq!(data(&fx, &["config", "list"])["config"], d["config"]);
}

#[test]
fn a4_config_set_get_unset_round_trip() {
    let fx = fixture();
    let set = data(&fx, &["config", "set", "update_check", "off"]);
    assert_eq!(set["changed"], true);
    assert_eq!(set["previous"], Value::Null);
    assert!(
        config_file(&fx).contains("update_check = off"),
        "{}",
        config_file(&fx)
    );

    let got = data(&fx, &["config", "get", "update_check"]);
    assert_eq!(
        (got["value"].as_str(), got["source"].as_str()),
        (Some("off"), Some("file"))
    );

    // the same value again writes nothing and says so
    let again = data(&fx, &["config", "set", "update_check", "off"]);
    assert_eq!(again["changed"], false);

    // other lines and comments in the file survive a later set
    let p = fx.cigscript().join("installer/config");
    let kept = format!("# keep me\n{}", config_file(&fx));
    std::fs::write(&p, kept).unwrap();
    data(&fx, &["config", "set", "keep_days", "30"]);
    let f = config_file(&fx);
    assert!(
        f.contains("# keep me") && f.contains("update_check = off") && f.contains("keep_days = 30"),
        "{f}"
    );

    let un = data(&fx, &["config", "unset", "update_check"]);
    assert_eq!(un["previous"], "off");
    assert_eq!(un["source"], "default");
    let got = data(&fx, &["config", "get", "update_check"]);
    assert_eq!(
        (got["value"].as_str(), got["source"].as_str()),
        (Some("on"), Some("default"))
    );
    assert!(!config_file(&fx).contains("update_check"));
}

#[test]
fn a4_config_refuses_a_bad_value_and_an_unknown_key_and_writes_nothing() {
    let fx = fixture();
    data(&fx, &["config", "set", "keep_days", "30"]);
    let before = config_file(&fx);

    for args in [
        ["config", "set", "update_check", "maybe"],
        ["config", "set", "keep_days", "soon"],
        ["config", "set", "no_such_key", "1"],
        ["config", "get", "no_such_key", ""],
    ] {
        let args: Vec<&str> = args.iter().copied().filter(|a| !a.is_empty()).collect();
        let o = cigi(&fx, &args);
        assert_eq!(o.status.code(), Some(3), "{args:?}: {}", text(&o));
        assert!(text(&o).contains("E1870"), "{args:?}: {}", text(&o));
    }
    assert_eq!(
        config_file(&fx),
        before,
        "a refused set must not touch the file"
    );
}

#[test]
fn a4_config_environment_beats_the_file() {
    let fx = fixture();
    data(&fx, &["config", "set", "update_check", "off"]);
    let run = |args: &[&str]| {
        let o = fx
            .cigi()
            .env("CIGI_UPDATE_CHECK", "on")
            .arg("--json")
            .args(args)
            .output()
            .unwrap();
        ok(&o);
        json(&o)["data"].clone()
    };
    let got = run(&["config", "get", "update_check"]);
    assert_eq!(
        (got["value"].as_str(), got["source"].as_str()),
        (Some("on"), Some("environment"))
    );
    // the file is still what it was, and `set` says the environment outranks it
    assert!(config_file(&fx).contains("update_check = off"));
    let o = fx
        .cigi()
        .env("CIGI_UPDATE_CHECK", "on")
        .args(["config", "set", "update_check", "off"])
        .output()
        .unwrap();
    ok(&o);
    assert!(text(&o).contains("CIGI_UPDATE_CHECK"), "{}", text(&o));
    // unset of a key only the environment holds is refused with its own code
    data(&fx, &["config", "unset", "update_check"]);
    let o = fx
        .cigi()
        .env("CIGI_UPDATE_CHECK", "on")
        .args(["config", "unset", "update_check"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(text(&o).contains("E1875"), "{}", text(&o));
}

#[test]
fn a4_config_unset_of_a_key_not_in_the_file_is_e1875() {
    let fx = fixture();
    let o = cigi(&fx, &["config", "unset", "theme"]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(text(&o).contains("E1875"), "{}", text(&o));
}

// ---------------------------------------------------------------- history

#[test]
fn a4_history_after_an_install_and_an_update_is_newest_first() {
    let mut fx = installed();
    fx.carton("0.1.1", &[("cig", "1.21.0"), ("cigo", "1.17.0")]);
    fx.publish("stable");
    ok(&cigi(&fx, &["update", "--no-trial"]));

    let d = data(&fx, &["history"]);
    let e = d["entries"].as_array().unwrap();
    assert_eq!(e.len(), 2, "{d}");
    assert_eq!(e[0]["what"], "update");
    assert_eq!(
        (e[0]["from"].as_str(), e[0]["to"].as_str()),
        (Some("0.1.0"), Some("0.1.1"))
    );
    assert_eq!(e[0]["ok"], true);
    assert_eq!(e[1]["what"], "install");
    assert_eq!(e[1]["to"], "0.1.0");
    assert!(e[0]["at"].as_str().unwrap() >= e[1]["at"].as_str().unwrap());
    assert!(!e[0]["id"].as_str().unwrap().is_empty());

    // the plain table names both and puts the update first
    let o = cigi(&fx, &["--plain", "history"]);
    ok(&o);
    let t = out(&o);
    let (u, i) = (t.find("update").unwrap(), t.find("install").unwrap());
    assert!(u < i, "{t}");
}

#[test]
fn a4_history_of_an_empty_home_is_empty_not_an_error() {
    let fx = fixture();
    let d = data(&fx, &["history"]);
    assert_eq!(d["entries"].as_array().unwrap().len(), 0);
}

// ---------------------------------------------------------------- shims

#[test]
fn a4_shims_sync_repairs_a_deleted_shim_a_stray_file_and_a_hand_edited_index() {
    let fx = installed();
    let bin = fx.cigscript().join("bin");
    let good_index = std::fs::read_to_string(index_path(&fx)).unwrap();
    let shim_ino = ino(&bin.join("cig"));
    assert_eq!(ino(&bin.join("cigo")), shim_ino);

    // `shims list` shows all of it as it stands
    let d = data(&fx, &["shims", "list"]);
    let names: Vec<&str> = d["shims"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["cig", "cigo"]);
    assert_eq!(d["index_matches_state"], true);
    assert!(
        d["shims"][0]["target"].as_str().unwrap().ends_with("cig"),
        "{d}"
    );

    // nothing drifted yet: sync changes nothing and says so
    let d = data(&fx, &["shims", "sync"]);
    assert_eq!(d["relinked"].as_array().unwrap().len(), 0, "{d}");
    assert_eq!(d["index_rewritten"], false);

    // damage three things
    std::fs::remove_file(bin.join("cig")).unwrap();
    std::fs::remove_file(bin.join("cigo")).unwrap();
    std::fs::write(bin.join("cigo"), "#!/bin/sh\necho stray\n").unwrap();
    std::fs::write(index_path(&fx), "hand edited\n").unwrap();

    let d = data(&fx, &["shims", "list"]);
    assert_eq!(d["index_matches_state"], false);
    assert_eq!(d["shims"][0]["present"], false);

    let d = data(&fx, &["shims", "sync"]);
    let mut relinked: Vec<&str> = d["relinked"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    relinked.sort();
    assert_eq!(relinked, ["cig", "cigo"]);
    assert_eq!(d["index_rewritten"], true);
    assert_eq!(ino(&bin.join("cig")), shim_ino);
    assert_eq!(
        ino(&bin.join("cigo")),
        shim_ino,
        "the stray file was replaced by the shim"
    );
    assert_eq!(
        std::fs::read_to_string(index_path(&fx)).unwrap(),
        good_index
    );

    // and the shim works again
    let o = fx.run_shim("cig", &fx.cigscript(), &["--version"]);
    assert!(
        o.status.success() || !text(&o).contains("E1840"),
        "{}",
        text(&o)
    );
}

#[test]
fn a4_shims_sync_with_nothing_installed_is_e1874() {
    let fx = fixture();
    let o = cigi(&fx, &["shims", "sync"]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(text(&o).contains("E1874"), "{}", text(&o));
}

// ---------------------------------------------------------------- status

#[test]
fn a4_status_of_an_empty_home_says_so() {
    let fx = fixture();
    let d = data(&fx, &["status"]);
    assert_eq!(d["carton"], Value::Null);
    assert_eq!(d["channel"], "stable");
    assert_eq!(d["waiting"]["update"], Value::Null);
    let o = cigi(&fx, &[]);
    ok(&o);
    assert!(out(&o).contains("nothing is installed"), "{}", text(&o));
}

#[test]
fn a4_status_json_has_the_documented_shape_and_a_bare_run_is_the_same_screen() {
    let fx = installed();
    let o = cigi(&fx, &["--json", "status"]);
    ok(&o);
    let j = json(&o);
    assert_eq!(j["command"], "status");
    let d = &j["data"];
    // docs/JSON.md: carton, channel, tools, trust, waiting{update, advisories, yanked}
    for k in ["carton", "channel", "tools", "trust", "waiting"] {
        assert!(d.get(k).is_some(), "{k} missing: {d}");
    }
    assert_eq!(d["carton"], "0.1.0");
    assert_eq!(d["channel"], "stable");
    assert_eq!(d["tools"]["cig"], "1.20.0");
    assert_eq!(d["tools"]["cigo"], "1.17.0");
    let trusted: u64 = d["trust"]
        .as_object()
        .unwrap()
        .values()
        .map(|v| v.as_u64().unwrap())
        .sum();
    assert_eq!(trusted, 2, "{d}");
    assert_eq!(d["waiting"]["update"], Value::Null);
    assert!(d["waiting"]["advisories"].as_array().unwrap().is_empty());
    assert!(d["waiting"]["yanked"].as_array().unwrap().is_empty());

    // a bare `ciginstaller` gives the same data under the same command name
    let bare = cigi(&fx, &["--json"]);
    ok(&bare);
    assert_eq!(json(&bare)["command"], "status");
    assert_eq!(json(&bare)["data"], *d);
}

#[test]
fn a4_status_shows_a_waiting_carton_in_at_most_eight_lines_without_the_network() {
    let mut fx = installed();
    fx.carton("0.1.1", &[("cig", "1.20.1"), ("cigo", "1.17.0")]);
    fx.publish("stable");
    // the cache learns about 0.1.1 the ordinary way
    ok(&cigi(&fx, &["update", "--check", "--refresh"]));

    // a mirror that cannot be reached: status must not need it
    let o = fx
        .cigi()
        .env("CIGI_MIRROR", "http://127.0.0.1:9/")
        .args(["--plain", "--offline", "status"])
        .output()
        .unwrap();
    ok(&o);
    let t = out(&o);
    assert!(t.lines().count() <= 8, "{t}");
    assert!(t.contains("0.1.1") && t.contains("cig 1.20.0"), "{t}");
    assert!(
        t.contains("ciginstaller update"),
        "the next step is update: {t}"
    );

    let d = data(&fx, &["--offline", "status"]);
    assert_eq!(d["waiting"]["update"], "0.1.1");
    assert_eq!(d["carton"], "0.1.0");
}

// ---------------------------------------------------------------- which, no tool

#[test]
fn a4_which_with_no_tool_lists_every_tool_and_why() {
    let fx = installed();
    let d = data(&fx, &["which", "--why"]);
    let rows = d["tools"].as_array().unwrap();
    assert_eq!(rows.len(), 4, "{d}");
    let row = |t: &str| rows.iter().find(|r| r["tool"] == t).unwrap();
    for t in ["cig", "cigo"] {
        let r = row(t);
        assert_eq!(r["carton"], "0.1.0");
        assert_eq!(r["why"], "the machine's carton");
        assert!(r["path"].as_str().unwrap().contains(t));
        assert!(
            !r["steps"].as_array().unwrap().is_empty(),
            "--why carries the steps: {r}"
        );
    }
    assert_eq!(row("cig")["version"], "1.20.0");
    // a tool no carton has still gets a row, with its code, and the command still answers
    assert!(
        row("cigar")["code"].as_str().unwrap().starts_with("E18"),
        "{}",
        row("cigar")
    );
    assert_eq!(row("cigar")["path"], Value::Null);

    let o = cigi(&fx, &["--plain", "which"]);
    ok(&o);
    assert!(out(&o).contains("2 of 4"), "{}", out(&o));
}

#[test]
fn a4_which_path_with_no_tool_is_a_usage_error() {
    let fx = installed();
    let o = cigi(&fx, &["which", "--path"]);
    assert_eq!(o.status.code(), Some(3), "{}", text(&o));
    assert!(text(&o).contains("E1870"), "{}", text(&o));
}
