// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

use cig_compat::{Pin, Version};

#[test]
fn version_parse_forms() {
    assert_eq!(Version::parse("1.2.3").unwrap(), Version::new(1, 2, 3));
    assert_eq!(Version::parse("v1.2.3").unwrap(), Version::new(1, 2, 3));
    let pre = Version::parse("1.2.3-beta.1").unwrap();
    assert_eq!(pre.pre.as_deref(), Some("beta.1"));
    assert_eq!(pre.to_string(), "1.2.3-beta.1");
    for bad in [
        "1.2", "1", "", "v", "1.2.3.4", "01.2.3", "1.2.3-", "1.2.3+b", "1.2.x", " 1.2.3",
        "1.2.3-01", "vv1.2.3",
    ] {
        assert!(Version::parse(bad).is_err(), "{bad:?} parsed");
    }
}

#[test]
fn version_ordering_with_prereleases() {
    // the ladder from the semver specification
    let ladder = [
        "1.0.0-alpha",
        "1.0.0-alpha.1",
        "1.0.0-alpha.beta",
        "1.0.0-beta",
        "1.0.0-beta.2",
        "1.0.0-beta.11",
        "1.0.0-rc.1",
        "1.0.0",
        "1.0.1",
        "1.1.0",
        "2.0.0",
    ];
    for w in ladder.windows(2) {
        let (a, b) = (Version::parse(w[0]).unwrap(), Version::parse(w[1]).unwrap());
        assert!(a < b, "{} < {}", w[0], w[1]);
    }
    assert!(Version::parse("1.10.0").unwrap() > Version::parse("1.9.0").unwrap());
    assert!(Version::parse("0.1.0-rc.1").unwrap() < Version::parse("0.1.0").unwrap());
}

/// (input, parses?, display, [(version, satisfied?)])
type Row = (&'static str, bool, &'static str);

pub const TABLE: &[Row] = &[
    ("1", true, "1"),
    ("v1", true, "1"),
    ("1.1", true, "1.1"),
    ("v1.1", true, "1.1"),
    ("1.1.2", true, "1.1.2"),
    ("v1.1.2", true, "1.1.2"),
    (">=1.1.2", true, ">=1.1.2"),
    (">= 1.1.2", true, ">=1.1.2"),
    (">=v1.1.2", true, ">=1.1.2"),
    (">=1.1", true, ">=1.1.0"),
    (">=1", false, ""),
    (" 1.1 ", true, "1.1"),
    ("0", true, "0"),
    ("0.1", true, "0.1"),
    ("0.1.2", true, "0.1.2"),
    (">=0.1.2", true, ">=0.1.2"),
    ("10.20.30", true, "10.20.30"),
    ("", false, ""),
    ("x", false, ""),
    ("1.", false, ""),
    (".1", false, ""),
    ("1..2", false, ""),
    ("1.1.1.1", false, ""),
    ("~1.1", false, ""),
    ("^1.1", false, ""),
    ("1.a", false, ""),
    ("1.1.2-beta", false, ""),
    ("v", false, ""),
    ("vv1", true, "1"),
    (">=", false, ""),
    (">=x", false, ""),
    (">=1.1.2-beta", true, ">=1.1.2"),
    (">=1.1.2+build", true, ">=1.1.2"),
    ("<=1.1", false, ""),
    ("=1.1", false, ""),
    ("1.1.x", false, ""),
    ("99999999999999999999", false, ""),
    ("-1", false, ""),
    ("1 .1", false, ""),
    ("+1", false, ""),
];

pub const PROBES: &[(u64, u64, u64)] = &[
    (0, 0, 0),
    (0, 1, 0),
    (0, 1, 2),
    (1, 0, 0),
    (1, 1, 0),
    (1, 1, 1),
    (1, 1, 2),
    (1, 1, 3),
    (1, 2, 0),
    (1, 9, 0),
    (2, 0, 0),
    (10, 20, 30),
];

#[test]
fn pin_table_shape() {
    assert_eq!(TABLE.len(), 40);
    for (input, ok, shown) in TABLE {
        match Pin::parse(input) {
            Ok(p) => {
                assert!(*ok, "{input:?} should not parse");
                assert_eq!(p.to_string(), *shown, "{input:?}");
            }
            Err(_) => assert!(!*ok, "{input:?} should parse"),
        }
    }
}

#[test]
fn pins_match_like_cig() {
    let p = Pin::parse("1.1").unwrap();
    assert!(p.satisfied_by(&Version::new(1, 1, 0)) && p.satisfied_by(&Version::new(1, 1, 9)));
    assert!(!p.satisfied_by(&Version::new(1, 2, 0)));
    let p = Pin::parse("v1.1.2").unwrap();
    assert!(
        p.is_exact()
            && p.satisfied_by(&Version::new(1, 1, 2))
            && !p.satisfied_by(&Version::new(1, 1, 3))
    );
    let p = Pin::parse(">= 1.1.2").unwrap();
    assert!(p.satisfied_by(&Version::new(1, 9, 0)) && !p.satisfied_by(&Version::new(1, 1, 1)));
    assert!(
        !p.satisfied_by(&Version::new(2, 0, 0)),
        "a pin never crosses a major version"
    );
    assert!(Pin::parse("1")
        .unwrap()
        .satisfied_by(&Version::new(1, 5, 0)));
}

#[test]
fn prereleases_never_satisfy_a_range() {
    let pre = Version::parse("1.1.2-rc.1").unwrap();
    for p in ["1", "1.1", ">=1.0.0"] {
        assert!(!Pin::parse(p).unwrap().satisfied_by(&pre), "{p}");
    }
    assert!(!Pin::parse("1.1.2").unwrap().satisfied_by(&pre));
}

#[test]
fn carton_lines() {
    assert_eq!(
        Pin::parse_carton_line("carton 0.1").unwrap(),
        Pin::Minor(0, 1)
    );
    assert_eq!(
        Pin::parse_carton_line("0.1.2").unwrap(),
        Pin::Exact(Version::new(0, 1, 2))
    );
    assert_eq!(
        Pin::parse_carton_line("carton >=0.1.2").unwrap(),
        Pin::AtLeast(Version::new(0, 1, 2))
    );
    assert!(Pin::parse_carton_line("cartons 0.1").is_err());
}

#[test]
fn floor() {
    assert_eq!(Pin::Minor(1, 2).floor(), Version::new(1, 2, 0));
    assert_eq!(Pin::Major(3).floor(), Version::new(3, 0, 0));
}

#[cfg(feature = "parity")]
#[test]
fn parity_with_cigscript_v1_20_0() {
    use cigscript::pin::Constraint;
    use cigscript::update::Version as Theirs;
    for (input, _, _) in TABLE {
        let (ours, theirs) = (Pin::parse(input), Constraint::parse(input));
        assert_eq!(ours.is_ok(), theirs.is_ok(), "{input:?}");
        let (Ok(o), Ok(t)) = (ours, theirs) else {
            continue;
        };
        assert_eq!(o.to_string(), t.to_string(), "{input:?}");
        assert_eq!(o.is_exact(), t.is_exact(), "{input:?}");
        for (a, b, c) in PROBES {
            assert_eq!(
                o.satisfied_by(&Version::new(*a, *b, *c)),
                t.satisfied_by(&Theirs(*a, *b, *c)),
                "{input:?} against {a}.{b}.{c}"
            );
        }
    }
}
