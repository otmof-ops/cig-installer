// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! Phase 4 acceptance (SPEC 14.3): `doctor`. Each finding is produced by a fault, ends with a
//! command, and that command, when run, clears it (the test runs it and asks doctor again).
//! Assertions are on codes, sections, severities and `--json` data, not on glyphs or labels.

mod common;

use cigi_testkit::{Fixture, Snapshot};
use common::{cigi, fixture, ok, text};
use serde_json::Value;

/// `Command::output`, tried again while exec fails with "text file busy" (ETXTBSY).
fn retrying(c: &mut std::process::Command) -> std::process::Output {
    for _ in 0..200 {
        match c.output() {
            Err(e) if e.raw_os_error() == Some(26) => {
                std::thread::sleep(std::time::Duration::from_millis(5))
            }
            r => return r.expect("run doctor"),
        }
    }
    panic!("text file busy for a second");
}

fn setup() -> Fixture {
    let mut fx = fixture();
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.16.0")]);
    fx.carton("0.2.0", &[("cig", "1.21.0"), ("cigo", "1.17.0")]);
    fx.publish("stable");
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    fx
}

/// `(exit code, the data object of --json doctor)`.
fn doctor_args(fx: &Fixture, path: Option<&str>, extra: &[&str]) -> (i32, Value) {
    let mut c = fx.cigi();
    c.env("CIGI_ALLOW_SINGLE_SOURCE", "true")
        .arg("--yes")
        .arg("--json")
        .arg("doctor")
        .args(extra);
    if let Some(p) = path {
        c.env("PATH", p);
    }
    let o = retrying(&mut c);
    let line = String::from_utf8_lossy(&o.stdout)
        .lines()
        .last()
        .unwrap_or("")
        .to_string();
    let v: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {}", text(&o)));
    (o.status.code().unwrap_or(-1), v["data"].clone())
}

fn doctor(fx: &Fixture) -> (i32, Value) {
    doctor_args(fx, None, &[])
}

fn findings<'a>(d: &'a Value, section: &str, sev: &str) -> Vec<&'a Value> {
    d["findings"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|f| f["section"] == section && f["severity"] == sev)
                .collect()
        })
        .unwrap_or_default()
}

fn clean(fx: &Fixture) {
    let (code, d) = doctor(fx);
    let acts: Vec<&Value> = d["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["severity"] == "action")
        .collect();
    assert!(acts.is_empty(), "still needs action: {acts:#?}");
    assert_eq!(code, 0);
}

/// Run the fix command a finding printed (`ciginstaller <args>`).
fn run_fix(fx: &Fixture, fix: &str) {
    let rest = fix.strip_prefix("ciginstaller ").expect(fix);
    let args: Vec<&str> = rest.split_whitespace().collect();
    ok(&cigi(fx, &args));
}

#[test]
fn a4_doctor_clean_home_is_read_only() {
    let fx = setup();
    let before = Snapshot::take(&fx.user_home);
    let (code, d) = doctor(&fx);
    assert_eq!(code, 0, "{d:#}");
    assert_eq!(d["needs_action"], 0);
    for s in [
        "transactions",
        "toolchains",
        "index",
        "shims",
        "PATH",
        "PATH block",
        "humidor",
        "disk",
    ] {
        assert!(!findings(&d, s, "ok").is_empty(), "no ok finding for {s}");
    }
    // the sections the build does not have are said to be absent
    assert!(!findings(&d, "not in this build", "note").is_empty());
    // every finding has a because; every action has a fix
    for f in d["findings"].as_array().unwrap() {
        assert!(!f["because"].as_str().unwrap_or("").is_empty(), "{f}");
    }
    // no lock, no journal, no cache write: the home is exactly as it was
    Snapshot::take(&fx.user_home).assert_same(&before);
    assert!(d["tools"].as_array().unwrap().is_empty());
}

#[test]
fn a4_doctor_nothing_installed() {
    let fx = fixture();
    let (code, d) = doctor(&fx);
    assert_eq!(code, 0);
    assert_eq!(d["needs_action"], 0);
    let n = findings(&d, "install", "note");
    assert_eq!(n.len(), 1);
    assert_eq!(n[0]["fix"], "ciginstaller install carton");
    assert!(!fx.cigscript().exists(), "doctor made the home");
}

#[test]
fn a4_doctor_deleted_shim() {
    let fx = setup();
    std::fs::remove_file(fx.cigscript().join("bin/cigo")).unwrap();
    let (code, d) = doctor(&fx);
    assert_eq!(code, 2, "{d:#}");
    let f = findings(&d, "shims", "action");
    assert_eq!(f.len(), 1, "{d:#}");
    assert_eq!(f[0]["code"], "E1844");
    assert!(f[0]["found"].as_str().unwrap().contains("cigo"));
    run_fix(&fx, f[0]["fix"].as_str().unwrap());
    clean(&fx);
    assert!(fx.cigscript().join("bin/cigo").exists());
}

#[test]
fn a4_doctor_tampered_toolchain_file() {
    let fx = setup();
    let file = fx.cigscript().join("toolchains/cigo/1.16.0/bin/cigo");
    let original = std::fs::read(&file).unwrap();
    // a tool that replaces its file by rename breaks only that link (SPEC 4.3)
    let tmp = file.with_file_name("cigo.new");
    std::fs::write(&tmp, b"#!/bin/sh\necho tampered\n").unwrap();
    std::fs::rename(&tmp, &file).unwrap();
    let (code, d) = doctor(&fx);
    assert_eq!(code, 2, "{d:#}");
    let f = findings(&d, "toolchains", "action");
    assert_eq!(f.len(), 1, "{d:#}");
    assert_eq!(f[0]["code"], "E1831");
    assert!(f[0]["found"].as_str().unwrap().contains("bin/cigo"));
    run_fix(&fx, f[0]["fix"].as_str().unwrap());
    clean(&fx);
    assert_eq!(std::fs::read(&file).unwrap(), original);
}

#[test]
fn a4_doctor_hand_edited_index() {
    let fx = setup();
    let idx = fx.cigscript().join("installer/shim-index");
    let original = std::fs::read(&idx).unwrap();
    let mut edited = original.clone();
    edited.extend_from_slice(b"pin\t9.9\tcigo\t/nowhere\n");
    std::fs::write(&idx, &edited).unwrap();
    let (code, d) = doctor(&fx);
    assert_eq!(code, 2, "{d:#}");
    let f = findings(&d, "index", "action");
    assert_eq!(f.len(), 1, "{d:#}");
    run_fix(&fx, f[0]["fix"].as_str().unwrap());
    clean(&fx);
    assert_eq!(
        std::fs::read(&idx).unwrap(),
        original,
        "regenerated byte for byte"
    );
    // a missing index is the same finding
    std::fs::remove_file(&idx).unwrap();
    let (_, d) = doctor(&fx);
    let f = findings(&d, "index", "action");
    assert_eq!(f.len(), 1, "{d:#}");
    assert_eq!(f[0]["fix"], "ciginstaller shims sync");
    run_fix(&fx, f[0]["fix"].as_str().unwrap());
    assert_eq!(std::fs::read(&idx).unwrap(), original);
}

#[test]
fn a4_doctor_interrupted_transaction() {
    let fx = setup();
    let o = fx
        .cigi()
        .env("CIGI_ALLOW_SINGLE_SOURCE", "true")
        .env("CIGI_FAULT", "link:3")
        .args(["--yes", "install", "carton", "--carton", "0.2.0"])
        .output()
        .unwrap();
    assert!(
        o.status.code().is_none(),
        "the install should have been killed"
    );
    let before = Snapshot::take(&fx.user_home);
    let (code, d) = doctor(&fx);
    assert_eq!(code, 2, "{d:#}");
    let f = findings(&d, "transactions", "action");
    assert_eq!(f.len(), 1, "{d:#}");
    assert_eq!(f[0]["code"], "E1832");
    // doctor only looks: it did not settle the transaction
    Snapshot::take(&fx.user_home).assert_same(&before);
    let o = cigi(&fx, &["repair"]);
    ok(&o);
    assert_eq!(text(&o).matches("E1832").count(), 1, "{}", text(&o));
    clean(&fx);
    assert!(!fx.cigscript().join("toolchains/cigo/1.17.0").exists());
}

// Unix only: a shell-script shadow earlier on PATH is reported with E1844 (PATH is `:`-separated).
#[cfg(unix)]
#[test]
fn a4_doctor_shadowing_binary_and_path() {
    let fx = setup();
    let bin = fx.cigscript().join("bin");
    let shadow = fx.tmp.path().join("shadow");
    std::fs::create_dir_all(&shadow).unwrap();
    let fake = shadow.join("cigo");
    std::fs::write(&fake, "#!/bin/sh\necho not ours\n").unwrap();
    std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let bad = format!("{}:{}:/usr/bin:/bin", shadow.display(), bin.display());
    let (code, d) = doctor_args(&fx, Some(&bad), &[]);
    assert_eq!(code, 2, "{d:#}");
    let f = findings(&d, "PATH", "action");
    assert_eq!(f.len(), 1, "{d:#}");
    assert_eq!(f[0]["code"], "E1844");
    assert!(f[0]["found"].as_str().unwrap().contains("cigo"));
    let fix = f[0]["fix"].as_str().unwrap().to_string();
    assert!(fix.starts_with("export PATH="), "{fix}");
    assert!(fix.contains(&bin.display().to_string()), "{fix}");
    // the fix puts bin first: apply it the way a shell would
    let fixed = format!("{}:{bad}", bin.display());
    let (code, d) = doctor_args(&fx, Some(&fixed), &[]);
    assert_eq!(code, 0, "{d:#}");
    // not on PATH at all
    let (code, d) = doctor_args(&fx, Some("/usr/bin:/bin"), &[]);
    assert_eq!(code, 2);
    assert_eq!(findings(&d, "PATH", "action")[0]["code"], "E1845");
}

#[test]
fn a4_doctor_path_block() {
    let fx = setup();
    let profile = fx.user_home.join(".profile");
    let original = std::fs::read_to_string(&profile).unwrap();
    assert!(original.contains("# >>> cigscript >>>"));
    // the block was deleted by hand: I know it was agreed to, so repair puts it back
    std::fs::write(&profile, "export FOO=1\n").unwrap();
    let (code, d) = doctor(&fx);
    assert_eq!(code, 2, "{d:#}");
    let f = findings(&d, "PATH block", "action");
    assert_eq!(f.len(), 1, "{d:#}");
    assert_eq!(f[0]["code"], "E1845");
    assert_eq!(f[0]["fix"], "ciginstaller repair");
    run_fix(&fx, "ciginstaller repair");
    clean(&fx);
    let t = std::fs::read_to_string(&profile).unwrap();
    assert!(t.starts_with("export FOO=1\n"), "{t}");
    assert_eq!(t.matches("# >>> cigscript >>>").count(), 1, "{t}");
    // an edited block is left alone, by repair too
    let edited = t.replace("export PATH=", "export  PATH=");
    std::fs::write(&profile, &edited).unwrap();
    let (code, d) = doctor(&fx);
    assert_eq!(code, 2);
    assert_eq!(findings(&d, "PATH block", "action")[0]["code"], "E1862");
    ok(&cigi(&fx, &["repair"]));
    assert_eq!(std::fs::read_to_string(&profile).unwrap(), edited);
}

#[test]
fn a4_doctor_respects_no_path() {
    let mut fx = fixture();
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.16.0")]);
    fx.publish("stable");
    ok(&cigi(&fx, &["install", "carton", "--no-path"]));
    let (code, d) = doctor(&fx);
    assert_eq!(code, 0, "{d:#}");
    assert_eq!(findings(&d, "PATH block", "note").len(), 1);
    assert!(findings(&d, "PATH block", "action").is_empty());
    let o = cigi(&fx, &["repair"]);
    ok(&o);
    assert!(
        !fx.user_home.join(".profile").exists(),
        "repair added a block that was declined"
    );
}

#[test]
fn a4_doctor_stale_and_expired_manifest() {
    let fx = setup();
    let meta = fx
        .cigscript()
        .join("installer/cache/manifests/stable/fetched.json");
    let original = std::fs::read_to_string(&meta).unwrap();
    let edit = |key: &str, to: &str| {
        let mut v: Value = serde_json::from_str(&original).unwrap();
        v[key] = Value::String(to.to_string());
        std::fs::write(&meta, v.to_string()).unwrap();
    };
    edit("fetched_at", "2020-01-01T00:00:00Z");
    let (code, d) = doctor(&fx);
    assert_eq!(code, 0, "an old cache is a note, not an action: {d:#}");
    let n = findings(&d, "manifest", "note");
    assert_eq!(n.len(), 1, "{d:#}");
    assert_eq!(n[0]["fix"], "ciginstaller update --check --refresh");
    run_fix(&fx, "ciginstaller update --check --refresh");
    let (_, d) = doctor(&fx);
    assert!(findings(&d, "manifest", "note").is_empty(), "{d:#}");
    std::fs::write(&meta, &original).unwrap();
    edit("expires", "2020-01-01T00:00:00Z");
    let (code, d) = doctor(&fx);
    assert_eq!(code, 2, "{d:#}");
    let f = findings(&d, "manifest", "action");
    assert_eq!(f[0]["code"], "E1823");
    run_fix(&fx, f[0]["fix"].as_str().unwrap());
    clean(&fx);
}

#[test]
fn a4_doctor_humidor_mismatch() {
    let fx = setup();
    let link = fx.cigscript().join("humidor/1.20.0/cig");
    assert!(link.exists());
    std::fs::remove_file(&link).unwrap();
    let (code, d) = doctor(&fx);
    assert_eq!(code, 2, "{d:#}");
    let f = findings(&d, "humidor", "action");
    assert_eq!(f.len(), 1, "{d:#}");
    run_fix(&fx, f[0]["fix"].as_str().unwrap());
    clean(&fx);
    assert!(link.exists());
    // and an entry that was dropped from the index
    let idx = fx.cigscript().join("humidor/index.json");
    let original = std::fs::read(&idx).unwrap();
    std::fs::write(&idx, "{\"entries\": []}\n").unwrap();
    let (_, d) = doctor(&fx);
    assert_eq!(findings(&d, "humidor", "action").len(), 1, "{d:#}");
    run_fix(&fx, "ciginstaller repair");
    clean(&fx);
    let back: Value = serde_json::from_slice(&std::fs::read(&idx).unwrap()).unwrap();
    let was: Value = serde_json::from_slice(&original).unwrap();
    assert_eq!(back["entries"][0]["version"], was["entries"][0]["version"]);
    assert_eq!(back["entries"][0]["sha256"], was["entries"][0]["sha256"]);
}

#[test]
fn a4_doctor_extra_file_in_a_toolchain() {
    let fx = setup();
    std::fs::write(fx.cigscript().join("toolchains/cig/1.20.0/bin/stray"), "x").unwrap();
    let (code, d) = doctor(&fx);
    assert_eq!(code, 2, "{d:#}");
    assert!(findings(&d, "toolchains", "action")[0]["found"]
        .as_str()
        .unwrap()
        .contains("stray"));
    run_fix(&fx, "ciginstaller repair");
    clean(&fx);
    assert!(!fx
        .cigscript()
        .join("toolchains/cig/1.20.0/bin/stray")
        .exists());
}

/// The tools' own doctors run only when asked, and print as they answered. The tools here are
/// the fixture's fakes, which echo their arguments.
#[test]
fn a4_doctor_tools_only_when_asked() {
    let fx = setup();
    let (_, plain) = doctor(&fx);
    assert!(plain["tools"].as_array().unwrap().is_empty());
    assert!(!plain.to_string().contains("args=doctor"));
    let (code, d) = doctor_args(&fx, None, &["--tools"]);
    assert_eq!(code, 0, "{d:#}");
    let tools = d["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 2, "{d:#}");
    let cig = tools.iter().find(|t| t["tool"] == "cig 1.20.0").unwrap();
    assert!(cig["output"]
        .as_str()
        .unwrap()
        .contains("cig 1.20.0 args=doctor"));
    assert_eq!(cig["ok"], true);
}

#[test]
fn a4_doctor_voice_in_plain_mode() {
    let fx = setup();
    std::fs::remove_file(fx.cigscript().join("bin/cigo")).unwrap();
    let o = cigi(&fx, &["--plain", "doctor"]);
    assert_eq!(o.status.code(), Some(2));
    let t = text(&o);
    assert!(t.contains("because I checked"), "{t}");
    assert!(t.contains("ciginstaller repair"), "{t}");
    assert!(t.contains("E1844"), "{t}");
}
