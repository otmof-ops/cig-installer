// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! Source naming, GitHub URL shapes, `source_from_config`, and the "only trust/ seals" rule.

use cigi_home::Config;
use cigi_source::{
    source_from_config, try_source_from_config, GithubSource, Location, ManifestPart, MirrorSource,
    Source,
};
use std::path::Path;

#[test]
fn github_urls_match_section_6() {
    let u = |l: &Location| GithubSource::url_for(l).unwrap().to_string();
    assert_eq!(
        u(&Location::Manifest { channel: "stable".into(), part: ManifestPart::Doc }),
        "https://github.com/otmof-ops/cig-installer/releases/download/manifest/manifest-stable.toml"
    );
    assert_eq!(
        u(&Location::Manifest { channel: "stable".into(), part: ManifestPart::Sidecar }),
        "https://github.com/otmof-ops/cig-installer/releases/download/manifest/manifest-stable.toml.sha256"
    );
    assert_eq!(
        u(&Location::Asset { repo: "otmof-ops/cig-script".into(), tag: "v1.20.0".into(), name: "cig-v1.20.0-linux-x86_64".into() }),
        "https://github.com/otmof-ops/cig-script/releases/download/v1.20.0/cig-v1.20.0-linux-x86_64"
    );
    assert_eq!(
        u(&Location::Installer { tag: "v0.1.0".into(), name: "ciginstaller-v0.1.0-linux-x86_64.tar.gz".into() }),
        "https://github.com/otmof-ops/cig-installer/releases/download/v0.1.0/ciginstaller-v0.1.0-linux-x86_64.tar.gz"
    );
    assert_eq!(
        u(&Location::Url {
            url: "https://github.com/a/b".into()
        }),
        "https://github.com/a/b"
    );
    // a manifest `url` cannot take the download to another host (a mirror serves it by name)
    assert!(GithubSource::url_for(&Location::Url {
        url: "https://example.com/a/b".into()
    })
    .is_err());
    assert!(GithubSource::url_for(&Location::Url {
        url: "http://example.com/a".into()
    })
    .is_err());
    let g = GithubSource;
    assert_eq!(g.name(), "github");
    assert!(g.independent());
}

#[test]
fn mirror_names_and_independence() {
    let d = MirrorSource::parse("/tmp/some-dir").unwrap();
    assert_eq!(d.name(), "dir:/tmp/some-dir/");
    assert!(!d.independent());
    let f = MirrorSource::parse("file:///tmp/some-dir").unwrap();
    assert!(f.name().starts_with("dir:/tmp/some-dir"));
    let h = MirrorSource::parse("https://mirror.example/cigi/").unwrap();
    assert_eq!(h.name(), "mirror:https://mirror.example/cigi/");
    assert!(!h.independent());
}

#[test]
fn source_from_config_picks_and_fails_loudly() {
    let none = |_: &str| None;
    let cfg = Config::from_text("", &none);
    assert_eq!(source_from_config(&cfg).name(), "github");
    let cfg = Config::from_text("mirror = file:///tmp/x\n", &none);
    assert!(source_from_config(&cfg).name().starts_with("dir:"));
    let cfg = Config::from_text("mirror = http://example.com/\n", &none);
    assert!(try_source_from_config(&cfg).is_err());
    // an unusable mirror never falls back to GitHub: every request is refused with the reason
    let s = source_from_config(&cfg);
    assert_eq!(s.name(), "invalid");
    let e = s
        .fetch_text(
            &Location::Manifest {
                channel: "stable".into(),
                part: ManifestPart::Doc,
            },
            10,
        )
        .unwrap_err();
    assert_eq!(e.code(), "E1820");
    assert!(e.to_string().contains("loopback"), "{e}");
    let cfg = Config::from_text("mirror = relative/dir\n", &none);
    assert!(try_source_from_config(&cfg).is_err());
}

#[test]
fn file_mirror_reads_and_reports_missing() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("manifest-stable.toml"), "hello").unwrap();
    let m = MirrorSource::parse(&d.path().display().to_string()).unwrap();
    let loc = |part| Location::Manifest {
        channel: "stable".into(),
        part,
    };
    assert_eq!(m.fetch_text(&loc(ManifestPart::Doc), 100).unwrap(), "hello");
    assert!(m.fetch_text(&loc(ManifestPart::Doc), 3).is_err());
    assert!(m
        .fetch_text(&loc(ManifestPart::Sidecar), 100)
        .unwrap_err()
        .is_not_found());
}

/// Section 2.3: only `crates/cigi-source/src/trust/` may call `seal`.
#[test]
fn only_the_trust_directory_seals() {
    fn walk(dir: &Path, hits: &mut Vec<String>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, hits);
            } else if p.extension().is_some_and(|x| x == "rs") {
                let text = std::fs::read_to_string(&p).unwrap();
                for (i, line) in text.lines().enumerate() {
                    let code = line.split("//").next().unwrap_or_default();
                    if code.contains("seal(") {
                        hits.push(format!("{}:{}", p.display(), i + 1));
                    }
                }
            }
        }
    }
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut hits = Vec::new();
    walk(&src, &mut hits);
    assert!(
        !hits.is_empty(),
        "the grep found no seal calls at all; the test is blind"
    );
    let outside: Vec<_> = hits.iter().filter(|h| !h.contains("/src/trust/")).collect();
    assert!(
        outside.is_empty(),
        "seal called outside trust/: {outside:?}"
    );
}
