// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! Regression tests from the correctness review of phase 1.

mod common;
use cig_compat::*;
use common::*;

#[test]
fn an_advisory_range_flags_a_prerelease_of_a_version_in_range() {
    let m = fleet();
    // `cig <1.7.0` (CIGI-2026-0002): 1.6.0-rc.1 is a 1.6.0 and is flagged
    let c = Combination {
        items: items(&[("cig", "1.6.0-rc.1")]),
        origin: Origin::Mixed,
    };
    let fs = check(&m, &c, &linux_host("6.1.0"));
    assert!(
        fs.iter()
            .any(|f| f.code == "E1804" && f.subject == "CIGI-2026-0002 cig 1.6.0-rc.1"),
        "{fs:#?}"
    );
    // a prerelease of a version outside the range is not
    let c = Combination {
        items: items(&[("cig", "1.8.0-rc.1")]),
        origin: Origin::Mixed,
    };
    assert!(!check(&m, &c, &linux_host("6.1.0"))
        .iter()
        .any(|f| f.code == "E1804"));
}

#[test]
fn matches_core_ignores_the_prerelease_but_matches_does_not() {
    let r = Requirement::parse(">=1.0.0, <1.4.2").unwrap();
    assert!(r.matches_core(&v("1.4.1-rc.2")));
    assert!(!r.matches(&v("1.4.1-rc.2"))); // semver's rule, kept for requirements
    assert!(!r.matches_core(&v("1.4.2-rc.1")));
    assert!(r.matches_core(&v("1.0.0")));
}

#[test]
fn only_an_exact_pin_falls_back_to_a_yanked_carton() {
    let m = fleet(); // carton 0.9.0 is the only 0.9.x and it is yanked
    assert!(m
        .newest_matching(&Pin::parse("0.9").unwrap(), None)
        .is_none());
    assert!(m.newest_matching(&Pin::parse("0").unwrap(), None).is_none());
    assert_eq!(
        m.newest_matching(&Pin::parse("0.9.0").unwrap(), None)
            .unwrap()
            .version,
        v("0.9.0")
    );
    // a live carton still wins for a series pin
    assert_eq!(
        m.newest_matching(&Pin::parse("1").unwrap(), None)
            .unwrap()
            .version,
        v("1.1.2")
    );
}
