// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! Plain is one resolved state (review fix 1): `explain` follows the same `Ui` as every other
//! command, so `CI`, `CIG_SCREEN_READER`, `voice = plain` and a non-English locale all give the
//! plain page, not only `--plain`. And `--plain`'s help names every cause (fix 14).

mod common;

use std::process::Output;

use cigi_testkit::Fixture;
use common::fixture;

/// A way to make the output plain: its name, extra arguments, extra environment.
type Cause<'a> = (&'a str, &'a [&'a str], &'a [(&'a str, &'a str)]);

fn explain(fx: &Fixture, extra_args: &[&str], env: &[(&str, &str)]) -> String {
    let mut c = fx.cigi();
    for k in [
        "CI",
        "CIG_PLAIN",
        "CIG_SCREEN_READER",
        "CIGI_VOICE",
        "LC_ALL",
        "LC_MESSAGES",
    ] {
        c.env_remove(k);
    }
    c.env("LANG", "en_AU.UTF-8");
    for (k, v) in env {
        c.env(k, v);
    }
    let o: Output = c
        .args(extra_args)
        .args(["explain", "E1811"])
        .output()
        .expect("run ciginstaller");
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).into_owned()
}

#[test]
fn explain_is_in_the_voice_when_nothing_says_plain() {
    let fx = fixture();
    let page = explain(&fx, &[], &[]);
    assert!(page.contains("can't see any ciggies bro"), "{page}");
}

#[test]
fn explain_is_plain_under_every_cause_of_plain() {
    let fx = fixture();
    let cases: [Cause; 6] = [
        ("--plain", &["--plain"], &[]),
        ("CI", &[], &[("CI", "true")]),
        ("CIG_SCREEN_READER", &[], &[("CIG_SCREEN_READER", "1")]),
        ("voice = plain", &[], &[("CIGI_VOICE", "plain")]),
        ("a German locale", &[], &[("LC_ALL", "de_DE.UTF-8")]),
        ("CIG_PLAIN", &[], &[("CIG_PLAIN", "1")]),
    ];
    for (what, args, env) in cases {
        let page = explain(&fx, args, env);
        assert!(
            !page.contains("ciggies"),
            "{what} did not give the plain page: {page}"
        );
        assert!(
            page.contains("wrong address"),
            "{what}: the manual's name: {page}"
        );
    }
}

#[test]
fn the_plain_flags_help_names_every_cause() {
    let fx = fixture();
    let o = fx.cigi().arg("--help").output().expect("run ciginstaller");
    let help = String::from_utf8_lossy(&o.stdout).replace('\n', " ");
    for needle in [
        "CI",
        "CIG_SCREEN_READER",
        "voice = plain",
        "non-English locale",
    ] {
        assert!(
            help.contains(needle),
            "--help does not mention `{needle}`: {help}"
        );
    }
}
