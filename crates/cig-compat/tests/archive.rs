// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! The archive index (#26): its shape, the manifest key that names it, and the keep rule.

use cig_compat::{prune_plan, ArchiveEntry, ArchiveIndex, ArchiveTag, Manifest, Version};
use std::collections::BTreeMap;

fn v(s: &str) -> Version {
    Version::parse(s).unwrap()
}

const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const COMMIT: &str = "3f9c3f9c3f9c3f9c3f9c3f9c3f9c3f9c3f9c3f9c";

pub fn index_json() -> String {
    format!(
        r#"{{"format": 1, "entries": [
 {{"product": "cig", "version": "1.6.2",
   "tag": {{"repo": "otmof-ops/cig-script-archive", "ref": "cig/v1.6.2", "commit": "{COMMIT}"}},
   "assets": [{{"name": "cig-v1.6.2-linux-x86_64",
     "url": "https://github.com/otmof-ops/cig-script-archive/releases/download/cig-v1.6.2/cig-v1.6.2-linux-x86_64",
     "sha256": "{SHA}"}}],
   "archived": "2026-11-12", "reason": "older than the three latest"}}]}}"#
    )
}

fn entry(version: &str) -> ArchiveEntry {
    ArchiveEntry {
        product: "cig".into(),
        version: v(version),
        tag: ArchiveTag {
            repo: "otmof-ops/cig-script-archive".into(),
            reference: format!("cig/v{version}"),
            commit: COMMIT.into(),
        },
        assets: vec![],
        archived: "2026-11-12".into(),
        reason: "older than the three latest".into(),
    }
}

#[test]
fn the_index_shape_of_the_issue_parses_and_round_trips() {
    let i = ArchiveIndex::from_json(&index_json()).unwrap();
    let e = i.find("cig", &v("1.6.2")).expect("entry");
    assert_eq!(e.tag.reference, "cig/v1.6.2");
    assert_eq!(e.asset("cig-v1.6.2-linux-x86_64").unwrap().sha256, SHA);
    assert_eq!(ArchiveIndex::from_json(&i.to_json()).unwrap(), i);
}

#[test]
fn a_malformed_index_is_refused_with_e1825() {
    let bad = [
        index_json().replace("\"format\": 1", "\"format\": 2"),
        index_json().replace(SHA, "ABC"),
        index_json().replace("https://github.com/", "http://example.com/"),
        index_json().replace("cig/v1.6.2\"", "../v1.6.2\""),
        index_json().replace(COMMIT, "3f9c"),
        "not json".to_string(),
    ];
    for b in bad {
        let e = ArchiveIndex::from_json(&b).unwrap_err();
        assert_eq!(e.code, "E1825", "{b}");
    }
    // the same version twice
    let one = ArchiveIndex::from_json(&index_json()).unwrap();
    let twice = ArchiveIndex {
        entries: vec![one.entries[0].clone(), one.entries[0].clone()],
    };
    assert!(ArchiveIndex::from_json(&twice.to_json()).is_err());
}

#[test]
fn the_manifest_may_name_an_archive_index() {
    let head = "format = 1\nchannel = \"stable\"\nsequence = 1\ngenerated = 2026-11-01T00:00:00Z\nexpires = 2026-12-01T00:00:00Z\n";
    let none = Manifest::from_str(head).unwrap();
    assert!(none.archive.is_none());
    let url = "https://github.com/otmof-ops/cig-script-archive/releases/download/index/index.json";
    let m = Manifest::from_str(&format!("{head}\n[archive]\nindex = \"{url}\"\n")).unwrap();
    assert_eq!(m.archive.as_ref().unwrap().index, url);
    assert!(m.warnings().is_empty());
    assert_eq!(Manifest::from_str(&m.render()).unwrap(), m);
    for bad in ["http://github.com/x", "https://example.com/x", "x y"] {
        let t = format!("{head}\n[archive]\nindex = \"{bad}\"\n");
        assert_eq!(Manifest::from_str(&t).unwrap_err().code, "E1825");
    }
}

#[test]
fn a_pinned_tag_is_never_archived() {
    // cig 1.0.0 .. 1.20.0; CigCompile's latest release still names v1.15.0
    let versions: Vec<Version> = (0..=20).map(|m| Version::new(1, m, 0)).collect();
    let mut pins = BTreeMap::new();
    pins.insert(
        v("1.15.0"),
        vec!["cig-compile 0.2.0 (Cargo tag)".to_string()],
    );
    let plan = prune_plan(&versions, 3, &pins);
    let row = |s: &str| plan.iter().find(|r| r.version == v(s)).unwrap();
    assert!(row("1.15.0").keep);
    assert!(row("1.15.0").reason.contains("pinned by cig-compile 0.2.0"));
    assert!(!row("1.14.0").keep);
    // and the index refuses to list it, whoever asks
    let by_product: BTreeMap<(String, Version), Vec<String>> = pins
        .into_iter()
        .map(|(ver, who)| (("cig".to_string(), ver), who))
        .collect();
    let mut index = ArchiveIndex::default();
    let refused = index.add(entry("1.15.0"), &by_product).unwrap_err();
    assert!(refused.contains("never archived"), "{refused}");
    assert!(index.entries.is_empty());
    index.add(entry("1.14.0"), &by_product).unwrap();
    assert!(index.add(entry("1.14.0"), &by_product).is_err());
}

#[test]
fn the_plan_gives_every_version_one_row_and_a_reason() {
    let versions: Vec<Version> = ["1.0.0", "1.2.0", "1.1.0", "1.3.0", "1.4.0", "1.2.0"]
        .iter()
        .map(|s| v(s))
        .collect();
    let plan = prune_plan(&versions, 3, &BTreeMap::new());
    let got: Vec<(String, bool)> = plan
        .iter()
        .map(|r| (r.version.to_string(), r.keep))
        .collect();
    assert_eq!(
        got,
        [
            ("1.4.0".to_string(), true),
            ("1.3.0".to_string(), true),
            ("1.2.0".to_string(), true),
            ("1.1.0".to_string(), false),
            ("1.0.0".to_string(), false),
        ]
    );
    assert!(plan.iter().all(|r| !r.reason.is_empty()));
}
