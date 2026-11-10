// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! Live checks against github.com, run by hand (CI runs no network tests):
//! `cargo test -p cigi-source --test live -- --ignored`. They prove the GitHub source reaches a
//! real release asset through its redirect rules, and that the bytes are the release's.

use cigi_source::{sidecar, GithubSource, Location, NoProgress, Source};

fn asset(name: &str) -> Location {
    Location::Asset {
        repo: "otmof-ops/cig-script".into(),
        tag: "v1.20.0".into(),
        name: name.into(),
    }
}

#[test]
#[ignore = "network: run by hand"]
fn a_real_release_asset_through_the_github_source() {
    let side = GithubSource
        .fetch_text(&asset("cig-v1.20.0-linux-x86_64.sha256"), 4096)
        .expect("the sidecar");
    let want = sidecar::parse(&side).expect("a digest");
    assert_eq!(
        want,
        "4649fda09236ada8a2ade2460c33db9cfa4b2f4dba40186099118a6a3b2dc8e3"
    );
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("cig");
    let info = GithubSource
        .fetch(&asset("cig-v1.20.0-linux-x86_64"), &dest, &mut NoProgress)
        .expect("the asset, through github.com's redirect to the asset host");
    assert_eq!(
        (info.sha256.as_str(), info.bytes),
        (want.as_str(), 13_010_720)
    );
}
