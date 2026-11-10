// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
#![allow(dead_code)]

use cig_compat::{Manifest, Version};
use cigi_source::{DigestTrust, MirrorSource, TrustLevel};
use cigi_testkit::Fixture;
use std::sync::Arc;

pub fn v(s: &str) -> Version {
    Version::parse(s).unwrap()
}

pub fn mirror(fx: &Fixture) -> MirrorSource {
    MirrorSource::parse(&fx.mirror_file_url()).unwrap()
}

/// Checksummed floor, no pins, no gh, and the user has accepted a single-operator mirror
/// (`allow_single_source = true`): the fixture mirror is one directory.
pub fn trust() -> DigestTrust {
    strict_trust().with_single_source(true)
}

/// As `trust()` but with the default `allow_single_source = false`.
pub fn strict_trust() -> DigestTrust {
    DigestTrust::new(TrustLevel::Checksummed)
        .with_pins(&[])
        .with_attest(Arc::new(cigi_source::NoAttest))
}

pub fn manifest_of(fx: &Fixture, channel: &str) -> Manifest {
    let text =
        std::fs::read_to_string(fx.releases.join(format!("manifest-{channel}.toml"))).unwrap();
    Manifest::from_str(&text).unwrap()
}

/// Everything in `cache/downloads/` (should be empty after a refusal).
pub fn downloads(fx: &Fixture) -> Vec<String> {
    let dir = fx.cigscript().join("installer/cache/downloads");
    match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// A source that must never be asked.
pub struct DeadSource;
impl cigi_source::Source for DeadSource {
    fn name(&self) -> String {
        "dead".into()
    }
    fn independent(&self) -> bool {
        false
    }
    fn fetch(
        &self,
        loc: &cigi_source::Location,
        _dest: &std::path::Path,
        _p: &mut dyn cigi_source::Progress,
    ) -> Result<cigi_source::source::FetchInfo, cigi_source::FetchError> {
        panic!("DeadSource asked for {loc:?}")
    }
    fn fetch_text(
        &self,
        loc: &cigi_source::Location,
        _max: usize,
    ) -> Result<String, cigi_source::FetchError> {
        panic!("DeadSource asked for {loc:?}")
    }
}
