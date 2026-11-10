// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! Accessibility (#31): text contrast over every token pair the UI uses, in both themes, and
//! the proof that `tokens.rs` is the file `design/tokens.json` describes.

use cigi_ui::detect::contrast;
use cigi_ui::tokens::{token, Pair, Role, Theme, BACKGROUND_DARK, BACKGROUND_LIGHT, PAIRS, ROLES};
use serde_json::Value;

fn hex((r, g, b): (u8, u8, u8)) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

#[test]
fn every_token_pair_used_meets_contrast() {
    assert!(PAIRS.len() >= 20, "the component list is not empty");
    let failing: Vec<String> = PAIRS
        .iter()
        .filter(|p: &&Pair| contrast(p.fg, p.bg) < p.min)
        .map(|p| format!("{}: {:.2} < {}", p.name, contrast(p.fg, p.bg), p.min))
        .collect();
    assert!(failing.is_empty(), "below WCAG AA: {failing:#?}");
}

#[test]
fn every_terminal_role_is_text_that_reads_on_its_theme() {
    for role in ROLES.iter().copied().filter(|r| *r != Role::Bold) {
        for (theme, bg) in [
            (Theme::Dark, BACKGROUND_DARK),
            (Theme::Light, BACKGROUND_LIGHT),
        ] {
            let ratio = contrast(token(role, theme).rgb, bg);
            assert!(
                ratio >= 4.5,
                "{} on {theme:?}: {ratio:.2}:1, text needs 4.5",
                role.name()
            );
        }
    }
}

#[test]
fn ember_is_never_text_on_light() {
    for role in [Role::Ember, Role::EmberBright, Role::EmberInk] {
        assert_eq!(
            hex(token(role, Theme::Light).rgb),
            "#c2410c",
            "{} on light is ember-ink",
            role.name()
        );
    }
    assert!(
        contrast((0xf9, 0x73, 0x16), BACKGROUND_LIGHT) < 3.0,
        "why: 2.80:1"
    );
}

#[test]
fn the_measured_values_in_issue_31_hold() {
    let night = BACKGROUND_DARK;
    let white = BACKGROUND_LIGHT;
    let c = |fg: (u8, u8, u8), bg| (contrast(fg, bg) * 100.0).round() / 100.0;
    assert_eq!(c((0xf9, 0x73, 0x16), night), 6.75, "ember on night-900");
    assert_eq!(c((0xfb, 0x92, 0x3c), night), 8.36, "ember-bright");
    assert_eq!(c((0xea, 0x58, 0x0c), night), 5.32, "ember-deep");
    assert_eq!(c((0xc8, 0xa2, 0x5f), night), 7.92, "filter");
    assert_eq!(c((0xd1, 0xd5, 0xdb), night), 12.84, "ash-300");
    assert_eq!(c((0x9c, 0xa3, 0xaf), night), 7.45, "ash-400");
    assert_eq!(c((0x6b, 0x72, 0x80), night), 3.91, "ash-500 on dark");
    assert_eq!(c((0x4a, 0xde, 0x80), night), 10.86, "ok on dark");
    assert_eq!(c((0xf9, 0x73, 0x16), white), 2.80, "ember on white");
    assert_eq!(c((0xea, 0x58, 0x0c), white), 3.56, "ember-deep on white");
    assert_eq!(c((0xc2, 0x41, 0x0c), white), 5.18, "ember-ink on white");
    assert_eq!(c((0x6b, 0x72, 0x80), white), 4.83, "ash-500 on white");
    assert_eq!(c((0x15, 0x80, 0x3d), white), 5.02, "ok on light");
    assert_eq!(
        c((0x2e, 0xa4, 0x4f), white),
        3.21,
        "the badges' green: badges only"
    );
    assert_eq!(
        c(night, (0xf9, 0x73, 0x16)),
        6.75,
        "night-900 on ember buttons"
    );
}

// ---- tokens.rs is what tokens.json says ----------------------------------------------

fn source() -> Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../design/tokens.json");
    let text = std::fs::read_to_string(path).expect("design/tokens.json");
    serde_json::from_str(&text).expect("tokens.json is JSON")
}

/// Follow `{a.b}` aliases to a `#rrggbb`; a value is a string or an object with `$value`.
fn resolve(root: &Value, v: &Value) -> String {
    let mut cur = v.clone();
    for _ in 0..8 {
        let s = cur
            .get("$value")
            .unwrap_or(&cur)
            .as_str()
            .expect("a string value")
            .to_string();
        match s.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
            Some(path) => {
                cur = path
                    .split('.')
                    .try_fold(root, |c, k| c.get(k))
                    .expect("alias target")
                    .clone();
            }
            None => return s,
        }
    }
    panic!("alias loop");
}

#[test]
fn tokens_rs_matches_tokens_json() {
    let root = source();
    let roles = &root["terminal"]["role"];
    for role in ROLES.iter().copied().filter(|r| *r != Role::Bold) {
        let def = &roles[role.name()];
        for (theme, key) in [(Theme::Dark, "dark"), (Theme::Light, "light")] {
            assert_eq!(
                hex(token(role, theme).rgb),
                resolve(&root, &def[key]),
                "{} {key}",
                role.name()
            );
        }
        assert_eq!(
            token(role, Theme::Dark).ansi16,
            def["ansi16"].as_str().unwrap()
        );
    }
    let names: Vec<&str> = roles
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    let have: Vec<&str> = ROLES.iter().map(|r| r.name()).collect();
    assert_eq!(
        have, names,
        "every role in the file is in the code, in order"
    );
    let pairs = root["pairs"]["list"].as_array().unwrap();
    assert_eq!(pairs.len(), PAIRS.len());
    for (p, want) in PAIRS.iter().zip(pairs) {
        assert_eq!(hex(p.fg), resolve(&root, &want["fg"]), "{}", p.name);
        assert_eq!(hex(p.bg), resolve(&root, &want["bg"]), "{}", p.name);
    }
    assert_eq!(
        cigi_ui::tokens::STANDARD_VERSION,
        root["version"].as_str().unwrap()
    );
}

#[test]
fn the_palette_is_issue_31s() {
    let t = |r, th| hex(token(r, th).rgb);
    assert_eq!(t(Role::Ok, Theme::Dark), "#4ade80");
    assert_eq!(t(Role::Ok, Theme::Light), "#15803d");
    assert_eq!(t(Role::Warn, Theme::Dark), "#fbbf24");
    assert_eq!(t(Role::Warn, Theme::Light), "#b45309");
    assert_eq!(t(Role::Danger, Theme::Dark), "#f87171");
    assert_eq!(t(Role::Danger, Theme::Light), "#b91c1c");
    assert_eq!(t(Role::Ember, Theme::Dark), "#f97316");
    assert_eq!(t(Role::EmberBright, Theme::Dark), "#fb923c");
    assert_eq!(t(Role::EmberInk, Theme::Light), "#c2410c");
    assert_eq!(
        token(Role::Ember, Theme::Dark).ansi256,
        208,
        "ember is 208 (#32)"
    );
    assert_eq!(
        token(Role::Ember, Theme::Dark).ansi16,
        "93",
        "bright yellow (#32)"
    );
}
