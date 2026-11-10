// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `tools_fall_back_to_the_index` (#26): a build whose release is gone is found in the archive,
//! and accepted only at the bytes the manifest pins.

use cig_compat::{ArchiveIndex, Manifest, Target, Version};
use cigi_source::{
    archived_locations, locate_with_index, FetchError, FetchInfo, Location, NoProgress, Progress,
    Source,
};
use std::collections::BTreeMap;
use std::path::Path;

const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const COMMIT: &str = "3f9c3f9c3f9c3f9c3f9c3f9c3f9c3f9c3f9c3f9c";
const ARCHIVED: &str =
    "https://github.com/otmof-ops/cig-script-archive/releases/download/cig-v1.6.2";

fn manifest() -> Manifest {
    Manifest::from_str(&format!(
        r#"format = 1
channel = "stable"
sequence = 1
generated = 2026-11-01T00:00:00Z
expires = 2026-12-01T00:00:00Z

[tool.cig]
repo = "otmof-ops/cig-script"
tag = "v{{version}}"
layout = "binary"
asset = "cig-v{{version}}-{{os}}-{{arch}}{{exe}}"
shims = [ {{ name = "cig", path = "bin/cig" }} ]
targets = [ "linux-x86_64" ]

[build.cig."1.6.2"."linux-x86_64"]
sha256 = "{SHA}"
size = 3
"#
    ))
    .unwrap()
}

fn index(sha: &str) -> ArchiveIndex {
    ArchiveIndex::from_json(&format!(
        r#"{{"format":1,"entries":[{{"product":"cig","version":"1.6.2",
        "tag":{{"repo":"otmof-ops/cig-script-archive","ref":"cig/v1.6.2","commit":"{COMMIT}"}},
        "assets":[{{"name":"cig-v1.6.2-linux-x86_64","url":"{ARCHIVED}/cig-v1.6.2-linux-x86_64","sha256":"{sha}"}}],
        "archived":"2026-11-12","reason":"older than the three latest"}}]}}"#
    ))
    .unwrap()
}

/// A source with only the files it is given, by URL; a repository release asks it in vain.
struct Fake {
    files: BTreeMap<String, Vec<u8>>,
}

impl Fake {
    fn get(&self, loc: &Location) -> Result<&Vec<u8>, FetchError> {
        match loc {
            Location::Url { url } => self
                .files
                .get(url)
                .ok_or(FetchError::NotFound { url: url.clone() }),
            other => Err(FetchError::NotFound {
                url: format!("{other:?}"),
            }),
        }
    }
}

impl Source for Fake {
    fn name(&self) -> String {
        "fake".into()
    }
    fn independent(&self) -> bool {
        true
    }
    fn fetch(
        &self,
        loc: &Location,
        dest: &Path,
        p: &mut dyn Progress,
    ) -> Result<FetchInfo, FetchError> {
        let bytes = self.get(loc)?;
        p.advance(bytes.len() as u64);
        std::fs::write(dest, bytes).unwrap();
        Ok(FetchInfo {
            bytes: bytes.len() as u64,
            sha256: String::new(),
        })
    }
    fn fetch_text(&self, loc: &Location, _max: usize) -> Result<String, FetchError> {
        Ok(String::from_utf8_lossy(self.get(loc)?).into_owned())
    }
}

fn archived_source() -> Fake {
    Fake {
        files: BTreeMap::from([
            (
                format!("{ARCHIVED}/cig-v1.6.2-linux-x86_64"),
                b"abc".to_vec(),
            ),
            (
                format!("{ARCHIVED}/cig-v1.6.2-linux-x86_64.sha256"),
                format!("{SHA}  cig-v1.6.2-linux-x86_64\n").into_bytes(),
            ),
        ]),
    }
}

#[test]
fn tools_fall_back_to_the_index() {
    let m = manifest();
    let art = m
        .artifact(
            "cig",
            &Version::parse("1.6.2").unwrap(),
            &Target::parse("linux-x86_64").unwrap(),
        )
        .unwrap();
    let src = archived_source();
    // the release is gone: without an index there is nowhere to look
    let got = locate_with_index(&src, art, || Ok(None)).unwrap();
    assert!(matches!(got.asset, Location::Asset { .. }));
    assert_eq!(got.sidecar_text, None);
    // with the index, the archive's URLs, and the archived sidecar read on the way
    let got = locate_with_index(&src, art, || Ok(Some(index(SHA)))).unwrap();
    let asset = got.asset;
    assert_eq!(
        asset,
        Location::Url {
            url: format!("{ARCHIVED}/cig-v1.6.2-linux-x86_64")
        }
    );
    assert_eq!(
        got.sidecar,
        Location::Url {
            url: format!("{ARCHIVED}/cig-v1.6.2-linux-x86_64.sha256")
        }
    );
    assert_eq!(
        got.sidecar_text.as_deref(),
        Some(format!("{SHA}  cig-v1.6.2-linux-x86_64\n").as_str())
    );
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("cig");
    src.fetch(&asset, &dest, &mut NoProgress).unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), b"abc");
}

#[test]
fn a_release_that_still_exists_is_asked_first() {
    let m = manifest();
    let art = m
        .artifact(
            "cig",
            &Version::parse("1.6.2").unwrap(),
            &Target::parse("linux-x86_64").unwrap(),
        )
        .unwrap();
    // the repository answers the sidecar, so the index is never consulted
    let (asset, sidecar) = cigi_source::fetch::artifact_locations(art);
    struct Live(Location);
    impl Source for Live {
        fn name(&self) -> String {
            "live".into()
        }
        fn independent(&self) -> bool {
            true
        }
        fn fetch(
            &self,
            _: &Location,
            _: &Path,
            _: &mut dyn Progress,
        ) -> Result<FetchInfo, FetchError> {
            unreachable!()
        }
        fn fetch_text(&self, loc: &Location, _: usize) -> Result<String, FetchError> {
            assert_eq!(loc, &self.0);
            Ok("live".into())
        }
    }
    let got = locate_with_index(&Live(sidecar.clone()), art, || {
        panic!("the index is loaded only when the repository has no sidecar")
    })
    .unwrap();
    assert_eq!((got.asset, got.sidecar), (asset, sidecar));
    assert_eq!(got.sidecar_text.as_deref(), Some("live"));
}

#[test]
fn an_index_that_lists_other_bytes_is_refused() {
    let m = manifest();
    let art = m
        .artifact(
            "cig",
            &Version::parse("1.6.2").unwrap(),
            &Target::parse("linux-x86_64").unwrap(),
        )
        .unwrap();
    let wrong = "b".repeat(64);
    let refused = archived_locations(art, &index(&wrong)).unwrap_err();
    assert!(matches!(refused, FetchError::Refused { .. }));
    assert!(locate_with_index(&archived_source(), art, || Ok(Some(index(&wrong)))).is_err());
    // an index that does not list the build leaves the repository's answer
    let empty = ArchiveIndex::default();
    assert_eq!(archived_locations(art, &empty).unwrap(), None);
}

/// `get_artifact` end to end: the manifest names an archive, the release is gone, and the build
/// comes from the archive at the bytes the manifest pins. A garbled index is refused (E1825).
#[test]
fn a_download_whose_release_is_gone_comes_from_the_archive() {
    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    const INDEX: &str =
        "https://github.com/otmof-ops/cig-installer/releases/download/archive/index.json";
    let text =
        manifest().render().replace(SHA, ABC) + &format!("\n[archive]\nindex = \"{INDEX}\"\n");
    let m = Manifest::from_str(&text).unwrap();
    assert_eq!(m.archive.as_ref().map(|a| a.index.as_str()), Some(INDEX));
    let trust = cigi_source::DigestTrust::new(cigi_source::TrustLevel::Checksummed)
        .with_pins(&[])
        .with_attest(std::sync::Arc::new(cigi_source::NoAttest))
        .with_single_source(true);
    let dir = tempfile::tempdir().unwrap();
    let home = cigi_home::Home::at(dir.path().to_path_buf());
    let cig = |src: &Fake| {
        cigi_source::get_artifact_for(
            src,
            &trust,
            &m,
            "cig",
            &Version::parse("1.6.2").unwrap(),
            &Target::parse("linux-x86_64").unwrap(),
            &home,
            &mut NoProgress,
        )
    };

    let mut src = archived_source();
    src.files.insert(
        format!("{ARCHIVED}/cig-v1.6.2-linux-x86_64.sha256"),
        format!("{ABC}  cig-v1.6.2-linux-x86_64\n").into_bytes(),
    );
    src.files
        .insert(INDEX.to_string(), index(ABC).to_json().into_bytes());
    let staged = cig(&src).unwrap();
    assert_eq!(std::fs::read(&staged.path).unwrap(), b"abc");
    assert_eq!(staged.art.sha256, ABC);

    src.files.insert(INDEX.to_string(), b"{not json".to_vec());
    let e = cig(&src).unwrap_err();
    assert_eq!(e.code(), "E1825", "{e}");
}
