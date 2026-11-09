// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0

use super::*;
use cigi_shim::pins::{pin_from_carton_file, pin_from_json, pin_from_toml};

const INDEX: &str = "cigi-index 1\nmachine\t0.1.0\n\
carton\t0.1.0\tcig\t/h/cig/1.20.0/cig\ncarton\t0.1.0\tcigo\t/h/cigo/1.17.0/cigo\n\
carton\t0.2.0\tcig\t/h/cig/1.21.0/cig\nstick\tcigcompile\t/h/cc/0.2.0/cigcompile\n\
carton-minor\t0.1\t0.1.0\ncarton-minor\t0.2\t0.2.0\ncarton-major\t0\t0.2.0\n\
legacy-pin-files\t.cig-version\t.cigar-version\n";

fn run(c: Option<&str>, p: &str) -> Pick {
    Pick::Run {
        carton: c.map(String::from),
        path: p.to_string(),
    }
}

#[test]
fn pin_file_dialects() {
    assert_eq!(
        pin_from_carton_file("# c\n\ncarton 0.1\n"),
        Some("0.1".into())
    );
    assert_eq!(pin_from_carton_file("0.1.2"), Some("0.1.2".into()));
    assert_eq!(
        pin_from_carton_file("carton >=0.1.2"),
        Some(">=0.1.2".into())
    );
    assert_eq!(pin_from_carton_file("# only a comment\n"), None);
    assert_eq!(
        pin_from_json(r#"{"a": {"carton": "9"}, "carton" : "0.1", "b": 1}"#),
        Some("0.1".into())
    );
    assert_eq!(pin_from_json(r#"{"a": {"carton": "9"}}"#), None);
    assert_eq!(
        pin_from_json(r#"{"x": ["carton"], "note": "carton: 1"}"#),
        None
    );
    assert_eq!(
        pin_from_toml("name = \"x\"\ncarton = '0.2'  # c\n[deps]\n"),
        Some("0.2".into())
    );
    assert_eq!(pin_from_toml("[p]\ncarton = \"0.2\"\n"), None);
    assert_eq!(pin_from_toml("cartonx = \"0.2\"\n"), None);
}

#[test]
fn resolution_table() {
    let carton = |s: &str| Pin::Carton(s.to_string());
    assert_eq!(
        pick(INDEX, "cig", None, &carton("0.1.0")),
        run(Some("0.1.0"), "/h/cig/1.20.0/cig")
    );
    assert_eq!(
        pick(INDEX, "cig", None, &carton("v0.2")),
        run(Some("0.2.0"), "/h/cig/1.21.0/cig")
    );
    assert_eq!(
        pick(INDEX, "cig", None, &carton("0")),
        run(Some("0.2.0"), "/h/cig/1.21.0/cig")
    );
    assert_eq!(pick(INDEX, "cig", None, &carton(">=0.1.2")), Pick::Slow);
    assert_eq!(pick(INDEX, "cig", None, &carton("0.3")), Pick::Slow);
    assert_eq!(
        pick(INDEX, "cigo", None, &carton("0.2")),
        Pick::Slow,
        "carton lacks the tool"
    );
    assert_eq!(
        pick(INDEX, "cig", None, &Pin::None),
        run(Some("0.1.0"), "/h/cig/1.20.0/cig")
    );
    assert_eq!(
        pick(INDEX, "cigcompile", None, &Pin::None),
        run(None, "/h/cc/0.2.0/cigcompile")
    );
    assert_eq!(pick(INDEX, "cigar", None, &Pin::None), Pick::Slow);
    assert_eq!(pick(INDEX, "cig", None, &Pin::Slow), Pick::Slow);
    assert_eq!(
        pick(INDEX, "cig", Some("0.2.0"), &carton("0.1")),
        run(Some("0.2.0"), "/h/cig/1.21.0/cig")
    );
    assert_eq!(pick(INDEX, "cig", Some("9.9.9"), &Pin::None), Pick::Slow);
    assert_eq!(pick("cigi-index 2\n", "cig", None, &Pin::None), Pick::Slow);
    assert_eq!(pick("", "cig", None, &Pin::None), Pick::Slow);
}

#[test]
fn legacy_list_is_read_from_the_index() {
    assert_eq!(legacy_names(INDEX), vec![".cig-version", ".cigar-version"]);
    assert!(legacy_names("cigi-index 1\n").is_empty());
}

#[test]
fn pin_grammar_is_cig_compats_digits_and_dots() {
    let carton = |s: &str| Pin::Carton(s.to_string());
    // what `Pin::parse` accepts runs in the shim too, `v` prefixes included
    for ok in ["0.1.0", "v0.1.0", "vv0.2", "0"] {
        assert_ne!(pick(INDEX, "cig", None, &carton(ok)), Pick::Slow, "{ok}");
    }
    // what it rejects goes to the resolver, which says why
    for bad in [
        "0.1.0-beta.1",
        "0.1.0+b",
        "0.1.",
        ".1",
        "0..1",
        "0.1.0.0",
        "0.x",
        "",
        ">=0.1",
    ] {
        assert_eq!(pick(INDEX, "cig", None, &carton(bad)), Pick::Slow, "{bad}");
    }
}
