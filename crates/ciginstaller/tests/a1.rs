// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0
//! Phase 1 acceptance (SPEC section 14.3): the manifest answers, the error pages, and a home
//! that is always a scratch one.

use cig_compat::{check, Combination, Host, Manifest, Origin, Severity, Target, Version};
use cigi_testkit::Fixture;

fn v(s: &str) -> Version {
    Version::parse(s).unwrap()
}

fn fixture_manifest() -> (Fixture, Manifest) {
    let mut fx = Fixture::new();
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.16.0")]);
    fx.carton("0.2.0", &[("cig", "1.20.0"), ("cigo", "1.17.0")]);
    let paths = fx.publish("stable");
    let text = std::fs::read_to_string(&paths.manifest).unwrap();
    let m = Manifest::from_str(&text).unwrap();
    (fx, m)
}

/// #2's done-when, the answering side: which build is in a carton, and whether a
/// combination fits.
#[test]
fn a1_manifest_answers() {
    let (_fx, m) = fixture_manifest();
    assert_eq!(m.build_in(&v("0.1.0"), "cigo"), Some(&v("1.16.0")));
    assert_eq!(m.build_in(&v("0.2.0"), "cigo"), Some(&v("1.17.0")));
    assert_eq!(m.build_in(&v("0.1.0"), "cigcompile"), None);

    let host = Host::from_target(&Target::host());
    // carton 0.1.0 as published fits itself
    let whole = Combination {
        items: vec![("cig".into(), v("1.20.0")), ("cigo".into(), v("1.16.0"))],
        origin: Origin::Carton(v("0.1.0")),
    };
    let refused: Vec<_> = check(&m, &whole, &host)
        .into_iter()
        .filter(|f| f.severity == Severity::Refuse)
        .collect();
    assert!(refused.is_empty(), "{refused:?}");
    // a stick of cigo 1.17.0 mixed into 0.1.0 is answered (fits or says why), never silent
    let stick = Combination {
        items: vec![("cig".into(), v("1.20.0")), ("cigo".into(), v("1.17.0"))],
        origin: Origin::Stick { carton: v("0.1.0") },
    };
    let findings = check(&m, &stick, &host);
    assert!(
        findings.iter().all(|f| !f.code.is_empty()),
        "every finding carries its code: {findings:?}"
    );
}

/// `explain` prints a code's page; an unknown code is a usage error (exit 3).
#[test]
fn a1_registry() {
    let fx = Fixture::new();
    let o = fx.cigi().args(["explain", "E1801"]).output().unwrap();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let page = String::from_utf8_lossy(&o.stdout);
    assert!(page.contains("E1801"), "{page}");

    let o = fx.cigi().args(["explain", "E9999"]).output().unwrap();
    assert_eq!(o.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&o.stderr).contains("E1870"));

    let o = fx
        .cigi()
        .args(["--json", "explain", "E1801"])
        .output()
        .unwrap();
    let obj: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(obj["command"], "explain");
    assert!(obj["page"].as_str().unwrap().contains("E1801"));
}

/// Every process a test starts gets a scratch HOME and CIGSCRIPT_HOME inside the fixture's
/// own folder, so no test can touch the real home.
#[test]
fn a1_home_is_scratch() {
    let fx = Fixture::new();
    let cmd = fx.cigi();
    let envs: std::collections::BTreeMap<_, _> = cmd
        .get_envs()
        .filter_map(|(k, v)| Some((k.to_string_lossy().into_owned(), v?.to_os_string())))
        .collect();
    let root = fx.tmp.path();
    for key in ["HOME", "CIGSCRIPT_HOME"] {
        let p = std::path::PathBuf::from(envs.get(key).unwrap_or_else(|| panic!("{key} unset")));
        assert!(
            p.starts_with(root),
            "{key} = {} is outside the fixture",
            p.display()
        );
    }
}
