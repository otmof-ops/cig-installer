// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! The compiled-in root and floor (decision 0004, section 5): until the first real root is filled
//! in, a `TufTrust` cannot be built from the compiled-in material, so nothing is trusted by default.

#![cfg(feature = "tuf")]
use cigi_source::trust::compiled_root;
use cigi_source::{tuf_floor, SeenState, Trust, TufConfig, TufTrust};

#[test]
fn the_compiled_root_is_either_a_signed_root_or_absent() {
    match compiled_root() {
        None => {
            let u = url::Url::parse("https://example.invalid/").unwrap();
            assert!(TufConfig::compiled(u, std::env::temp_dir()).is_none());
        }
        Some(b) => {
            let v: serde_json::Value = serde_json::from_slice(b).unwrap();
            assert_eq!(v["signed"]["_type"], "root");
            assert!(v["signatures"].as_array().is_some_and(|s| !s.is_empty()));
        }
    }
}

#[test]
fn the_floor_is_at_least_one() {
    const { assert!(tuf_floor::MIN_SNAPSHOT_VERSION >= 1) };
    const { assert!(tuf_floor::MIN_TARGETS_VERSION >= 1) };
}

#[test]
fn a_garbage_root_refuses_everything() {
    let d = tempfile::tempdir().unwrap();
    let t = TufTrust::new(TufConfig {
        root: b"not a root".to_vec(),
        metadata_url: url::Url::from_directory_path(d.path()).unwrap(),
        datastore: d.path().join("ds"),
        min_snapshot_version: 1,
        min_targets_version: 1,
    });
    let s = SeenState::default();
    assert!(t
        .admit_manifest(b"channel = \"stable\"", None, None, &s)
        .is_err());
}
