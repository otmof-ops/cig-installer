// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `a7_humidor_store` (ADR 0001 section 3): the objects live in `humidor/store/`. A home made by
//! the old layout, with its objects in `store/`, is read where it is by a command that only
//! reads, and moved once by the first command that writes.
#![cfg(unix)]

mod common;
use std::os::unix::fs::MetadataExt;

use common::*;

#[test]
fn a7_old_home_reads_in_place_and_moves_at_the_first_write() {
    let mut fx = fixture();
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.16.0")]);
    fx.publish("stable");
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    let root = fx.cigscript();
    // a fresh install keeps its objects under the humidor
    assert!(root.join("humidor/store/objects").is_dir());
    assert!(!root.join("store").exists());

    // make it a home of the old layout
    std::fs::rename(root.join("humidor/store"), root.join("store")).unwrap();
    let count = |p: &std::path::Path| -> usize {
        std::fs::read_dir(p.join("objects"))
            .unwrap()
            .flat_map(|s| std::fs::read_dir(s.unwrap().path()).unwrap())
            .count()
    };
    let n = count(&root.join("store"));
    assert!(n > 0);
    let shim_ino = std::fs::metadata(root.join("toolchains/cigo/1.16.0/bin/cigo"))
        .unwrap()
        .ino();

    // reading changes nothing
    let o = cigi(&fx, &["--json", "doctor"]);
    assert!(
        o.status.code().is_some_and(|c| c <= 1),
        "doctor: {}",
        text(&o)
    );
    assert!(root.join("store/objects").is_dir());
    assert!(!root.join("humidor/store").exists());
    let o = cigi(&fx, &["--json", "check"]);
    assert!(
        o.status.code().is_some_and(|c| c <= 1),
        "check: {}",
        text(&o)
    );
    assert!(root.join("store/objects").is_dir());
    assert!(!root.join("humidor/store").exists());

    // the first command that writes moves the store, and says so
    let o = cigi(&fx, &["gc"]);
    ok(&o);
    assert!(
        text(&o).contains("moved the store into the humidor"),
        "{}",
        text(&o)
    );
    assert!(!root.join("store").exists());
    assert_eq!(count(&root.join("humidor/store")), n);
    assert_eq!(
        std::fs::metadata(root.join("toolchains/cigo/1.16.0/bin/cigo"))
            .unwrap()
            .ino(),
        shim_ino,
        "the toolchain file is still the same inode"
    );
    // and the home is healthy in its new layout
    let o = cigi(&fx, &["--json", "doctor"]);
    let d: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(d["data"]["needs_action"], 0, "{}", text(&o));
    // a second write has nothing to move
    let o = cigi(&fx, &["gc"]);
    ok(&o);
    assert!(!text(&o).contains("moved the store"), "{}", text(&o));
}
