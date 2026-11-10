// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `generated_files_match_tokens` (#31): `crates/cigi-ui/src/tokens.rs` and `design/generated/`
//! are rebuilt from `design/tokens.json` and must equal what is on disk.

use std::path::{Path, PathBuf};
use xtask::tokens::{generate, run};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

fn source() -> String {
    std::fs::read_to_string(root().join("design/tokens.json")).expect("design/tokens.json")
}

#[test]
fn generated_files_match_tokens() {
    if let Err(e) = run(&root(), true) {
        panic!("{e}");
    }
}

#[test]
fn every_derived_file_is_built() {
    let paths: Vec<&str> = generate(&source())
        .expect("generates")
        .iter()
        .map(|o| o.path)
        .collect();
    for want in [
        "crates/cigi-ui/src/tokens.rs",
        "design/generated/tokens.css",
        "design/generated/egui-theme.rs",
        "design/generated/terminal-palettes.json",
        "design/generated/svg-styles.css",
        "design/generated/badges.json",
    ] {
        assert!(paths.contains(&want), "{want} is generated");
    }
}

#[test]
fn a_changed_token_changes_the_generated_files_so_drift_is_caught() {
    let edited = source().replacen("#4ade80", "#4ade81", 1);
    assert_ne!(edited, source());
    let on_disk = |p: &str| std::fs::read_to_string(root().join(p)).unwrap_or_default();
    let changed: Vec<&str> = generate(&edited)
        .expect("generates")
        .iter()
        .filter(|o| on_disk(o.path) != o.text)
        .map(|o| o.path)
        .collect();
    assert!(
        changed.contains(&"crates/cigi-ui/src/tokens.rs"),
        "{changed:?}"
    );
    assert!(
        changed.contains(&"design/generated/tokens.css"),
        "{changed:?}"
    );
}

#[test]
fn generation_is_deterministic_and_licensed() {
    let a = generate(&source()).expect("generates");
    let b = generate(&source()).expect("generates");
    for (x, y) in a.iter().zip(&b) {
        assert_eq!(x.text, y.text, "{}", x.path);
        if x.path.ends_with(".json") {
            continue; // JSON has no comments; REUSE.toml covers design/**
        }
        assert!(
            x.text.contains("SPDX-License-Identifier"),
            "{} has a header",
            x.path
        );
        assert!(
            x.text.contains("@generated"),
            "{} says it is generated",
            x.path
        );
    }
}

#[test]
fn nearest_256_puts_ember_on_208() {
    let t = generate(&source()).expect("generates");
    let rs = &t[0].text;
    assert!(
        rs.contains("(Theme::Dark, Role::Ember) => t(0xf9, 0x73, 0x16, 208, \"93\")"),
        "ember is 208 on 256-colour terminals (#32)"
    );
}

#[test]
fn a_broken_source_is_an_error_not_a_panic() {
    assert!(generate("{").is_err());
    assert!(generate("{}").is_err());
    let dangling = source().replacen("{color.ember-ink}", "{color.nope}", 1);
    assert!(
        generate(&dangling).is_err(),
        "an alias to nothing is refused"
    );
    let bad = source().replacen("#c2410c", "orange", 1);
    assert!(generate(&bad).is_err(), "a colour must be #rrggbb");
}
