// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0
//! `a3_check_matrix` (SPEC 14.3): one test per row of #4's table, calling the engine with the
//! row's facts, and the CLI's `check` output for three of them (plus the clean machine).

mod common;
use cig_compat::*;
use common::*;

const HEAD: &str = r#"
format = 1
channel = "stable"
sequence = 3
generated = 2026-11-01T00:00:00Z
expires = 2026-12-01T00:00:00Z

[tool.cig]
repo = "otmof-ops/cig-script"
tag = "v{version}"
layout = "binary"
asset = "cig-v{version}-{os}-{arch}{exe}"
shims = [ { name = "cig", path = "bin/cig" } ]
targets = [ "linux-x86_64", "macos-aarch64" ]

[tool.cigo]
repo = "otmof-ops/cig-orchestrator"
tag = "v{version}"
layout = "archive"
asset = "cig-orchestrator-v{version}.tar.gz"
strip = 1
shims = [ { name = "cigo", path = "bin/cigo" } ]
targets = [ "any" ]
"#;

fn build(tool: &str, version: &str, target: &str) -> String {
    format!(
        "[build.{tool}.\"{version}\"]\n[build.{tool}.\"{version}\".{target}]\nsha256 = \"{}\"\nsize = 1000\n",
        "a".repeat(64)
    )
}

/// A small channel with a yanked carton (0.9.0), two live ones (1.0.0, 1.1.2), requirements, an
/// advisory pair, journal formats and an OS minimum.
fn fleet() -> Manifest {
    let mut s = String::from(HEAD);
    for ver in ["1.6.0", "1.7.2", "1.8.0"] {
        s.push_str(&build("cig", ver, "linux-x86_64"));
    }
    for ver in ["1.3.0", "1.4.0"] {
        s.push_str(&build("cigo", ver, "any"));
    }
    s.push_str(
        r#"
[carton."0.9.0"]
cig = "1.6.0"
cigo = "1.3.0"
[carton."1.0.0"]
cig = "1.7.2"
cigo = "1.3.0"
[carton."1.1.2"]
cig = "1.8.0"
cigo = "1.4.0"

[product.cigo."1.4.0"]
requires = { cig = ">=1.8" }
[product.cigo."1.3.0"]
requires = { cig = ">=1.6, <2" }
recommends = { cig = ">=1.7" }
[product.cig."1.6.0"]
embeds = { journal_format = 2 }
[product.cig."1.7.2"]
embeds = { journal_format = 2 }
[product.cig."1.8.0"]
embeds = { journal_format = 3 }
os_min = { linux = "kernel 5.4", windows = "10" }

[yanked]
"carton 0.9.0" = { reason = "rollback skipped a file", fixed_in = "1.0.0" }
"cigo 1.4.1" = { reason = "bad launcher", fixed_in = "1.4.2" }

[[advisory]]
id = "CIGI-2026-0001"
affects = [ "carton <0.9.5" ]
fixed_in = "1.0.0"
severity = "security"
summary = "an example advisory"
url = "https://example.invalid/advisory"

[[advisory]]
id = "CIGI-2026-0002"
affects = [ "cig <1.7.0" ]
fixed_in = "1.7.0"
severity = "bug"
summary = "another example"
"#,
    );
    Manifest::from_str(&s).expect("the fleet parses")
}

fn v(s: &str) -> Version {
    Version::parse(s).expect("a version")
}

fn items(list: &[(&str, &str)]) -> Vec<(String, Version)> {
    list.iter().map(|(t, x)| (t.to_string(), v(x))).collect()
}

fn host(kernel: &str) -> Host {
    Host {
        os: "linux".into(),
        arch: "x86_64".into(),
        os_version: None,
        kernel: Some(kernel.into()),
    }
}

fn combo(list: &[(&str, &str)], origin: Origin) -> Combination {
    Combination {
        items: items(list),
        origin,
    }
}

fn only(fs: &[Finding], code: &str) -> Vec<Finding> {
    fs.iter().filter(|f| f.code == code).cloned().collect()
}

#[test]
fn a3_check_matrix_clean_carton() {
    let c = combo(
        &[("cig", "1.8.0"), ("cigo", "1.4.0")],
        Origin::Carton(v("1.1.2")),
    );
    assert_eq!(check(&fleet(), &c, &host("6.17.0-1033-oem")), vec![]);
}

#[test]
fn a3_check_matrix_e1800_pins_that_no_version_satisfies() {
    let pins = vec![
        ("cigo".to_string(), Pin::parse("1.3").unwrap()),
        ("cigo".to_string(), Pin::parse(">=1.4.0").unwrap()),
    ];
    let fs = check_pins(
        &fleet(),
        &pins,
        &items(&[("cigo", "1.4.0")]),
        &host("6.1.0"),
    );
    let f = &only(&fs, "E1800")[0];
    assert_eq!(f.severity, Severity::Refuse);
    assert!(f.headline.contains("pins disagree"), "{}", f.headline);
    // a pin the installed set misses, with a carton that has it listed: a warning and the move
    let pins = vec![("cigo".to_string(), Pin::parse("1.3").unwrap())];
    let fs = check_pins(
        &fleet(),
        &pins,
        &items(&[("cig", "1.8.0"), ("cigo", "1.4.0")]),
        &host("6.1.0"),
    );
    let f = &only(&fs, "E1800")[0];
    assert_eq!(f.severity, Severity::Warn);
    assert_eq!(fix_command(&f.fixes[0]), "ciginstaller carton 1.0.0");
}

#[test]
fn a3_check_matrix_e1801_a_product_needs_another() {
    let c = combo(&[("cig", "1.7.2"), ("cigo", "1.4.0")], Origin::Mixed);
    let fs = only(&check(&fleet(), &c, &host("6.1.0")), "E1801");
    assert_eq!(fs.len(), 1);
    assert_eq!(fs[0].severity, Severity::Refuse);
    assert_eq!(
        fs[0].headline,
        "cigo 1.4.0 needs cig 1.8 or newer; you have 1.7.2"
    );
    assert_eq!(fix_command(&fs[0].fixes[0]), "ciginstaller update");
    // a recommendation is only a warning
    let c = combo(
        &[("cig", "1.6.0"), ("cigo", "1.3.0")],
        Origin::Carton(v("0.9.0")),
    );
    let fs = only(&check(&fleet(), &c, &host("6.1.0")), "E1801");
    assert_eq!(fs[0].severity, Severity::Warn);
}

#[test]
fn a3_check_matrix_e1802_a_stick_does_not_fit_its_carton() {
    let c = combo(
        &[("cig", "1.8.0"), ("cigo", "1.3.0")],
        Origin::Stick { carton: v("1.1.2") },
    );
    let fs = only(&check(&fleet(), &c, &host("6.1.0")), "E1802");
    assert_eq!(fs.len(), 1);
    assert_eq!(fs[0].severity, Severity::Warn);
    assert_eq!(fs[0].subject, "cigo");
    assert_eq!(fix_command(&fs[0].fixes[0]), "ciginstaller carton 1.1.2");
}

#[test]
fn a3_check_matrix_e1803_yanked() {
    let c = combo(
        &[("cig", "1.6.0"), ("cigo", "1.3.0")],
        Origin::Carton(v("0.9.0")),
    );
    let fs = only(&check(&fleet(), &c, &host("6.1.0")), "E1803");
    assert_eq!(fs.len(), 1);
    assert_eq!(fs[0].severity, Severity::Warn);
    assert!(fs[0].headline.ends_with("rollback skipped a file"));
    assert_eq!(fix_command(&fs[0].fixes[0]), "ciginstaller carton 1.0.0");
}

#[test]
fn a3_check_matrix_e1804_advisory() {
    let c = combo(
        &[("cig", "1.6.0"), ("cigo", "1.3.0")],
        Origin::Carton(v("0.9.0")),
    );
    let fs = only(&check(&fleet(), &c, &host("6.1.0")), "E1804");
    let subjects: Vec<&str> = fs.iter().map(|f| f.subject.as_str()).collect();
    assert_eq!(
        subjects,
        ["CIGI-2026-0001 carton 0.9.0", "CIGI-2026-0002 cig 1.6.0"]
    );
    assert!(fs.iter().all(|f| f.severity == Severity::Warn));
}

#[test]
fn a3_check_matrix_e1805_compiled_program() {
    let m = fleet();
    // an older cig than the one the program embeds
    let fs = check_program(&m, &v("1.8.0"), 3, &v("1.7.2"));
    let f = &only(&fs, "E1805")[0];
    assert_eq!(f.severity, Severity::Refuse);
    assert!(f.headline.contains("older"), "{}", f.headline);
    // another major version
    let fs = check_program(&m, &v("1.8.0"), 3, &v("2.0.0"));
    assert!(only(&fs, "E1805")[0].headline.contains("different major"));
    // a cig that is new enough: no E1805
    assert!(only(&check_program(&m, &v("1.7.0"), 2, &v("1.8.0")), "E1805").is_empty());
}

#[test]
fn a3_check_matrix_e1806_journal_format() {
    let m = fleet();
    let newer = check_journal(&m, 4, &v("1.8.0"));
    assert_eq!(
        (newer[0].code, newer[0].severity),
        ("E1806", Severity::Refuse)
    );
    assert_eq!(fix_command(&newer[0].fixes[0]), "ciginstaller update");
    assert!(check_journal(&m, 3, &v("1.8.0")).is_empty());
    assert_eq!(
        check_journal(&m, 2, &v("1.8.0"))[0].severity,
        Severity::Note
    );
    // a cig the manifest knows nothing about is a note, never a guess
    assert_eq!(
        check_journal(&m, 3, &v("9.9.9"))[0].severity,
        Severity::Note
    );
}

#[test]
fn a3_check_matrix_e1807_os_minimum() {
    let c = combo(&[("cig", "1.8.0")], Origin::Carton(v("1.1.2")));
    let fs = only(&check(&fleet(), &c, &host("4.19.0-12-generic")), "E1807");
    assert_eq!(fs.len(), 1);
    assert_eq!(fs[0].severity, Severity::Refuse);
    assert!(fs[0].headline.contains("kernel 5.4"), "{}", fs[0].headline);
    assert!(only(&check(&fleet(), &c, &host("5.4.0")), "E1807").is_empty());
    // no kernel known: not evaluated
    let mut h = host("1.0");
    h.kernel = None;
    assert!(only(&check(&fleet(), &c, &h), "E1807").is_empty());
}

#[test]
fn a3_check_matrix_e1808_package_requirements() {
    let have = items(&[("cig", "1.8.0"), ("cigcompile", "0.2.0")]);
    assert!(check_package("pkg", &[("cigcompile".into(), ">=0.2".into())], &have).is_empty());
    let low = check_package("pkg", &[("cigcompile".into(), ">=0.3".into())], &have);
    assert_eq!((low[0].code, low[0].severity), ("E1808", Severity::Refuse));
    assert_eq!(
        low[0].headline,
        "pkg needs cigcompile 0.3 or newer; you have 0.2.0"
    );
    let none = check_package("pkg", &[("cigarscript".into(), ">=0.1".into())], &have);
    assert!(none[0].headline.contains("none is installed"));
}

#[test]
fn a3_check_matrix_e1811_e1814_e1815_the_plumbing_rows() {
    let m = fleet();
    let h = host("6.1.0");
    let c = combo(&[("cig", "1.8.0")], Origin::Carton(v("7.0.0")));
    assert_eq!(
        only(&check(&m, &c, &h), "E1811")[0].severity,
        Severity::Refuse
    );
    let c = combo(&[("cigar", "0.1.0")], Origin::Mixed);
    assert_eq!(
        only(&check(&m, &c, &h), "E1814")[0].severity,
        Severity::Warn
    );
    let mut win = host("1.0");
    win.os = "windows".into();
    let c = combo(&[("cig", "1.8.0")], Origin::Carton(v("1.1.2")));
    assert_eq!(
        only(&check(&m, &c, &win), "E1815")[0].severity,
        Severity::Refuse
    );
}

// ---------------------------------------------------------------- the CLI

fn cli_fixture() -> cigi_testkit::Fixture {
    let mut fx = fixture();
    fx.product("cigo", "1.17.0").requires("cig", ">=1.21");
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.16.0")]);
    fx.carton("0.2.0", &[("cig", "1.20.0"), ("cigo", "1.17.0")]);
    fx.carton("0.3.0", &[("cig", "1.20.0"), ("cigo", "1.16.0")]);
    fx.yank_carton("0.3.0", "rollback skipped a file");
    fx.publish("stable");
    fx
}

#[test]
fn a3_check_cli_clean_machine() {
    let fx = cli_fixture();
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    let o = cigi(&fx, &["check"]);
    ok(&o);
    assert!(
        text(&o).contains("this machine (carton 0.1.0): every combination checks out"),
        "{}",
        text(&o)
    );
    // nothing installed: a note and the way in
    let empty = cli_fixture();
    let o = cigi(&empty, &["check"]);
    ok(&o);
    assert!(text(&o).contains("nothing is installed"), "{}", text(&o));
}

#[test]
fn a3_check_cli_e1801_refuses_with_exit_2() {
    let fx = cli_fixture();
    let o = cigi(&fx, &["check", "--carton", "0.2.0"]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    let t = text(&o);
    assert!(t.contains("E1801"), "{t}");
    assert!(
        t.contains("cigo 1.17.0 needs cig 1.21 or newer; you have 1.20.0"),
        "{t}"
    );
    assert!(t.contains("1 refused"), "{t}");
    // the same through --json: every finding with its code and fixes
    let o = cigi(&fx, &["--json", "check", "--carton", "0.2.0"]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    let j: serde_json::Value = serde_json::from_slice(&o.stdout).expect("one JSON object");
    assert_eq!(j["ok"], false);
    let f = &j["data"]["findings"][0];
    assert_eq!(f["code"], "E1801");
    assert_eq!(f["severity"], "refuse");
    assert_eq!(f["subject"], "cigo");
    assert!(f["fixes"].as_array().is_some_and(|a| !a.is_empty()));
    assert_eq!(j["data"]["worst"], "refuse");
}

#[test]
fn a3_check_cli_e1803_a_yanked_carton_warns_and_exits_0() {
    let fx = cli_fixture();
    let o = cigi(&fx, &["check", "--carton", "0.3.0"]);
    ok(&o);
    let t = text(&o);
    assert!(t.contains("E1803"), "{t}");
    assert!(t.contains("rollback skipped a file"), "{t}");
    assert!(t.contains("nothing refused"), "{t}");
}

#[test]
fn a3_check_cli_e1811_no_such_carton_refuses() {
    let fx = cli_fixture();
    let o = cigi(&fx, &["check", "--carton", "7.0.0"]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(text(&o).contains("E1811"), "{}", text(&o));
    // a usage mistake is exit 3
    let o = cigi(&fx, &["check", "--carton", "banana"]);
    assert_eq!(o.status.code(), Some(3), "{}", text(&o));
}

#[test]
fn a3_check_cli_project_pins_are_checked_against_what_is_installed() {
    let fx = cli_fixture();
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    // a carton pin to a carton that is on the channel but not installed: a note, exit 0
    std::fs::write(fx.project.join(".cig-carton"), "carton 0.2\n").unwrap();
    let o = cigi(&fx, &["check", "--project", fx.project.to_str().unwrap()]);
    ok(&o);
    assert!(text(&o).contains("is not installed"), "{}", text(&o));
    // a pin no carton satisfies: refused
    std::fs::write(fx.project.join(".cig-carton"), "carton 9.9\n").unwrap();
    let o = cigi(&fx, &["check", "--project", fx.project.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(text(&o).contains("E1811"), "{}", text(&o));
}

/// SEC-004 finding 6: a different document at a sequence already seen is not read from the
/// cache by anyone (`check`, the resolver and the switch checks all go through it), and a fetch
/// that meets one puts the cache back as it was, so a yank the first document carried stays known.
#[test]
fn a3_check_replayed_manifest_never_unyanks() {
    let mut fx = fixture();
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.16.0")]);
    fx.carton("0.2.0", &[("cig", "1.21.0"), ("cigo", "1.17.0")]);
    fx.yank_carton("0.2.0", "rollback skipped a file");
    fx.publish("stable"); // sequence 1, with the yank
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    let cache = fx.cigscript().join("installer/cache/manifests/stable");
    let first = std::fs::read(cache.join("manifest.toml")).unwrap();
    assert!(String::from_utf8_lossy(&first).contains("rollback skipped a file"));

    // the mirror now serves another document at the same sequence, without the yank
    fx.carton("0.2.0", &[("cig", "1.21.0"), ("cigo", "1.17.0")]);
    fx.publish_with(
        "stable",
        &cigi_testkit::PublishOpts {
            sequence: Some(1),
            ..Default::default()
        },
    );
    let o = cigi(&fx, &["install", "carton", "--carton", "0.2.0"]);
    assert!(!o.status.success(), "{}", text(&o));
    assert!(text(&o).contains("E1824"), "{}", text(&o));
    // the fetch wrote the replay and put the first document back
    assert_eq!(std::fs::read(cache.join("manifest.toml")).unwrap(), first);
    let o = cigi(&fx, &["--offline", "check", "--carton", "0.2.0"]);
    ok(&o);
    assert!(text(&o).contains("E1803"), "{}", text(&o));

    // and a replay that is already in the cache is not read (the old hole, made by hand)
    let new = std::fs::read(fx.releases.join("manifest-stable.toml")).unwrap();
    let hex = cigi_testkit::sha256_hex(&new);
    std::fs::write(cache.join("manifest.toml"), &new).unwrap();
    std::fs::write(
        cache.join("manifest.toml.sha256"),
        format!("{hex}  manifest-stable.toml\n"),
    )
    .unwrap();
    let mut meta: serde_json::Value =
        serde_json::from_slice(&std::fs::read(cache.join("fetched.json")).unwrap()).unwrap();
    meta["sha256"] = hex.into();
    std::fs::write(cache.join("fetched.json"), meta.to_string()).unwrap();
    let o = cigi(&fx, &["--offline", "check", "--carton", "0.2.0"]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(text(&o).contains("E1824"), "{}", text(&o));
}
