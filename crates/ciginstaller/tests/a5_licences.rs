// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `a5_licences`: `ciginstaller licences`, the per-build license field, and the license display
//! at the first install (#27). Assertions are on codes, data and the state file, not on icons.
#![cfg(unix)]

mod common;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use cigi_testkit::Fixture;
use common::*;
use serde_json::Value;

const MIRROR: (&str, &str) = ("CIGI_ALLOW_SINGLE_SOURCE", "true");
/// What the fixture's archive products carry as `LICENSE`.
const LICENSE_FILE: &[u8] = b"Apache-2.0 (fixture)\n";

fn json_of(o: &Output) -> Value {
    serde_json::from_slice(&o.stdout).unwrap_or_else(|e| panic!("{e}: {}", text(o)))
}

fn data(fx: &Fixture, args: &[&str]) -> Value {
    let mut a = vec!["--json"];
    a.extend_from_slice(args);
    let o = cigi(fx, &a);
    ok(&o);
    json_of(&o)["data"].clone()
}

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
fn on_tty(fx: &Fixture, args: &str, keys: &[&str]) -> (i32, String) {
    let c = fx.cigi();
    let exe = c.get_program().to_string_lossy().to_string();
    let q = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
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
    type_at_prompts(s, keys)
}

fn state(fx: &Fixture) -> Value {
    let t = std::fs::read_to_string(fx.home.state_path()).expect("state.json");
    serde_json::from_str(&t).expect("state.json is JSON")
}

fn receipt(fx: &Fixture, tool: &str, version: &str) -> Value {
    let p = fx
        .cigscript()
        .join("toolchains")
        .join(tool)
        .join(version)
        .join(".cigi-receipt.json");
    serde_json::from_str(&std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{p:?}: {e}")))
        .expect("receipt is JSON")
}

/// cig 1.20.0 (Apache-2.0 in the manifest) and cigo 1.17.0 (no license stated, a LICENSE file
/// in its archive), published as carton 0.1.0.
fn stated() -> Fixture {
    let mut fx = fixture();
    fx.product("cig", "1.20.0").license("Apache-2.0");
    fx.product("cigo", "1.17.0");
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.17.0")]);
    fx.publish("stable");
    fx
}

fn build<'a>(d: &'a Value, tool: &str) -> &'a Value {
    d["builds"]
        .as_array()
        .and_then(|b| b.iter().find(|b| b["tool"] == tool))
        .unwrap_or_else(|| panic!("no {tool} in {d}"))
}

// ---------------------------------------------------------------- the field

#[test]
fn a5_the_license_goes_from_the_manifest_into_the_receipt() {
    let fx = stated();
    ok(&cigi(&fx, &["install", "carton", "--no-path"]));
    assert_eq!(receipt(&fx, "cig", "1.20.0")["license"], "Apache-2.0");
    // no license stated: the key is absent, not an empty value
    assert!(receipt(&fx, "cigo", "1.17.0").get("license").is_none());
    // and the installer's own receipt carries its license
    let own = receipt(&fx, "ciginstaller", env!("CARGO_PKG_VERSION"));
    assert_eq!(own["license"], "FSL-1.1-ALv2");
}

#[test]
fn a5_licences_lists_the_installer_and_each_build_with_its_license() {
    let fx = stated();
    ok(&cigi(&fx, &["install", "carton", "--no-path"]));
    let d = data(&fx, &["licences"]);
    assert_eq!(d["installer"]["license"], "FSL-1.1-ALv2");
    assert_eq!(d["installer"]["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(
        d["installer"]["spdx_urls"][0],
        "https://spdx.org/licenses/FSL-1.1-ALv2.html"
    );
    assert_eq!(
        d["installer"]["text_sha256"].as_str().map(str::len),
        Some(64)
    );
    let cig = build(&d, "cig");
    assert_eq!(cig["version"], "1.20.0");
    assert_eq!(cig["license"], "Apache-2.0");
    assert_eq!(
        cig["spdx_urls"][0],
        "https://spdx.org/licenses/Apache-2.0.html"
    );
    assert_eq!(cig["from"][0], "carton 0.1.0");
    // cig is a single binary, so there is no license file inside it, but the receipt says where
    // the build came from
    assert!(cig["text_path"].is_null());
    assert!(cig["source"]["repo"].is_string());
    // a build with no stated license says null (the words are "not stated" on the page)
    let cigo = build(&d, "cigo");
    assert!(cigo["license"].is_null());
    let path = cigo["text_path"].as_str().expect("cigo carries a LICENSE");
    assert!(
        Path::new(path).is_absolute() && path.ends_with("LICENSE"),
        "{path}"
    );
    assert_eq!(
        std::fs::read(path).expect("the file is there"),
        LICENSE_FILE
    );
}

#[test]
fn a5_the_page_says_not_stated_and_names_the_terms_record() {
    let fx = stated();
    ok(&cigi(&fx, &["install", "carton", "--no-path"]));
    let o = fx
        .cigi()
        .env(MIRROR.0, MIRROR.1)
        .args(["licences", "--plain"])
        .output()
        .unwrap();
    ok(&o);
    let t = text(&o);
    assert!(t.contains("not stated"), "{t}");
    assert!(
        t.contains("FSL-1.1-ALv2") && t.contains("Apache-2.0"),
        "{t}"
    );
    assert!(t.contains("terms:"), "{t}");
    assert!(t.contains("ciginstaller licences"), "{t}");
}

#[test]
fn a5_a_manifest_without_the_field_works_and_a_pre_field_receipt_says_not_stated() {
    // no license anywhere: the manifest of every carton so far
    let mut fx = fixture();
    fx.carton("0.1.0", &[("cig", "1.20.0")]);
    fx.publish("stable");
    ok(&cigi(&fx, &["install", "carton", "--no-path"]));
    let d = data(&fx, &["licences"]);
    assert!(build(&d, "cig")["license"].is_null());
    // a receipt written before the field existed has no key either
    let fx = stated();
    ok(&cigi(&fx, &["install", "carton", "--no-path"]));
    let p = fx
        .cigscript()
        .join("toolchains/cig/1.20.0/.cigi-receipt.json");
    let mut r: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
    r.as_object_mut().unwrap().remove("license");
    std::fs::write(&p, serde_json::to_string_pretty(&r).unwrap()).unwrap();
    let d = data(&fx, &["licences"]);
    assert!(build(&d, "cig")["license"].is_null());
    // and the rest of the installer still reads that receipt
    ok(&cigi(&fx, &["doctor"]));
}

#[test]
fn a5_licences_on_an_empty_machine_lists_the_installer_and_touches_nothing() {
    let fx = fixture();
    let d = data(&fx, &["licences", "--offline"]);
    assert_eq!(d["installer"]["license"], "FSL-1.1-ALv2");
    assert_eq!(d["builds"].as_array().map(Vec::len), Some(0));
    assert!(d["terms"].is_null());
}

// ---------------------------------------------------------------- the text

#[test]
fn a5_text_prints_the_license_file_as_it_is() {
    let fx = stated();
    ok(&cigi(&fx, &["install", "carton", "--no-path"]));
    // the installer's: the repository's LICENSE, byte for byte
    let o = cigi(&fx, &["licences", "ciginstaller", "--text"]);
    ok(&o);
    let repo = std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../LICENSE")).unwrap();
    assert_eq!(
        o.stdout.strip_suffix(b"\n").unwrap_or(&o.stdout),
        repo.strip_suffix(b"\n").unwrap_or(&repo)
    );
    // a build's: the file inside its install
    let o = cigi(&fx, &["licences", "cigo", "--text"]);
    ok(&o);
    assert!(out(&o).starts_with("Apache-2.0 (fixture)"), "{}", text(&o));
}

#[test]
fn a5_text_errors_have_their_codes() {
    let fx = stated();
    ok(&cigi(&fx, &["install", "carton", "--no-path"]));
    // a build with no license file inside it: E1878
    let o = cigi(&fx, &["--json", "licences", "cig", "--text"]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    let j = json_of(&o);
    assert_eq!(j["error"]["code"], "E1878");
    assert_eq!(j["error"]["hint"], "ciginstaller licences");
    // a tool nothing holds: E1877
    let o = cigi(&fx, &["--json", "licences", "nope"]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert_eq!(json_of(&o)["error"]["code"], "E1877");
    // --text with no tool is a usage error
    let o = cigi(&fx, &["--json", "licences", "--text"]);
    assert_eq!(o.status.code(), Some(3), "{}", text(&o));
    assert_eq!(json_of(&o)["error"]["code"], "E1870");
}

// ---------------------------------------------------------------- the first-run display

#[test]
fn a5_yes_acknowledges_and_records_the_terms_once() {
    let fx = stated();
    let d = data(&fx, &["install", "carton", "--no-path"]);
    assert_eq!(d["terms"]["how"], "acknowledged-by-flag");
    let t = &state(&fx)["terms"];
    assert_eq!(t["carton"], "0.1.0");
    assert_eq!(t["how"], "acknowledged-by-flag");
    assert_eq!(
        t["licenses"],
        serde_json::json!(["Apache-2.0", "FSL-1.1-ALv2"])
    );
    assert_eq!(
        t["installer_license_sha256"].as_str().map(str::len),
        Some(64)
    );
    assert!(t["at"].as_str().is_some_and(|a| a.ends_with('Z')), "{t}");
    // `licences` shows what was recorded
    let d = data(&fx, &["licences"]);
    assert_eq!(d["terms"]["carton"], "0.1.0");
    // the same licenses in a later carton are not shown again, and the record stays as it was
    let mut fx = fx;
    fx.carton("0.2.0", &[("cig", "1.20.0"), ("cigo", "1.17.0")]);
    fx.publish("stable");
    let d = data(
        &fx,
        &["install", "carton", "--carton", "0.2.0", "--no-path"],
    );
    assert!(d["terms"].is_null(), "{d}");
    assert_eq!(state(&fx)["terms"]["carton"], "0.1.0");
}

#[test]
fn a5_a_license_the_machine_has_not_seen_is_shown_again() {
    let mut fx = stated();
    ok(&cigi(&fx, &["install", "carton", "--no-path"]));
    fx.product("cig", "1.21.0").license("MIT");
    fx.carton("0.2.0", &[("cig", "1.21.0"), ("cigo", "1.17.0")]);
    fx.publish("stable");
    let d = data(
        &fx,
        &["install", "carton", "--carton", "0.2.0", "--no-path"],
    );
    assert_eq!(d["terms"]["carton"], "0.2.0");
    assert_eq!(
        state(&fx)["terms"]["licenses"],
        serde_json::json!(["FSL-1.1-ALv2", "MIT"])
    );
}

#[test]
fn a5_with_no_terminal_and_no_yes_the_terms_are_noted_not_confirmed() {
    let fx = stated();
    let o = bare(
        &fx,
        &fx.project,
        &["--json", "install", "carton", "--no-path"],
    );
    ok(&o);
    let j = json_of(&o);
    assert_eq!(j["data"]["terms"]["how"], "acknowledged-without-terminal");
    let line = j["detail"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .find(|l| l.starts_with("licenses:"))
        .expect("a licenses line")
        .to_string();
    assert!(
        line.contains("not confirmed") && line.contains("ciginstaller licences"),
        "{line}"
    );
    assert_eq!(state(&fx)["terms"]["how"], "acknowledged-without-terminal");
}

#[test]
fn a5_a_terminal_asks_once_and_a_no_installs_nothing() {
    let fx = stated();
    // a first install also asks the preset and crash-report questions: enter takes each default.
    // n = no to the licenses: nothing fetched, nothing recorded
    let (code, t) = on_tty(&fx, "install carton --no-path", &["\n", "\n", "n"]);
    assert_eq!(code, 2, "{t}");
    assert!(t.contains("E1876"), "{t}");
    assert!(t.contains("have you seen these licenses?"), "{t}");
    assert!(
        t.contains("FSL-1.1-ALv2") && t.contains("Apache-2.0"),
        "{t}"
    );
    assert!(t.contains("ciginstaller licences"), "{t}");
    let s = state_or_empty(&fx);
    assert!(s["terms"].is_null() && s["machine_carton"].is_null(), "{s}");
    assert!(!fx.cigscript().join("toolchains/cig").exists());
    // enter takes the default, which is no
    let (code, t) = on_tty(&fx, "install carton --no-path", &["\n", "\n", "\n"]);
    assert_eq!(code, 2, "{t}");
    assert!(state_or_empty(&fx)["terms"].is_null());
    // y = yes: installed, recorded as answered by a person
    let (code, t) = on_tty(&fx, "install carton --no-path", &["\n", "\n", "y"]);
    assert_eq!(code, 0, "{t}");
    assert_eq!(state(&fx)["terms"]["how"], "acknowledged");
    // the second install on the same machine asks nothing: there is no prompt to type at
    let mut fx = fx;
    fx.carton("0.2.0", &[("cig", "1.20.0"), ("cigo", "1.17.0")]);
    fx.publish("stable");
    let (code, t) = on_tty(&fx, "install carton --carton 0.2.0 --no-path", &[]);
    assert_eq!(code, 0, "{t}");
    assert!(!t.contains("have you seen these licenses?"), "{t}");
}

fn state_or_empty(fx: &Fixture) -> Value {
    match std::fs::read_to_string(fx.home.state_path()) {
        Ok(t) => serde_json::from_str(&t).expect("state.json is JSON"),
        Err(_) => serde_json::json!({ "terms": null, "machine_carton": null }),
    }
}

#[test]
fn a5_a_dry_run_shows_nothing_and_records_nothing() {
    let fx = stated();
    let d = data(&fx, &["install", "carton", "--dry-run"]);
    assert_eq!(d["dry_run"], true);
    assert!(state_or_empty(&fx)["terms"].is_null());
}

#[test]
fn a5_the_terms_go_nowhere() {
    // the fixture's server records every request: `licences` makes none
    let fx = stated();
    ok(&cigi(&fx, &["install", "carton", "--no-path"]));
    let server = fx.serve();
    let before = server.requests().len();
    let o = fx
        .cigi_with_mirror(&server.url())
        .env(MIRROR.0, MIRROR.1)
        .args(["licences"])
        .output()
        .unwrap();
    ok(&o);
    assert_eq!(server.requests().len(), before, "licences made a request");
}
