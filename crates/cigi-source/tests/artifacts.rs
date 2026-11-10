// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! Every trust rule for artifacts, driven through the fixture mirror (file source, no network).

mod common;
use cig_compat::Target;
use cigi_source::progress::CountingProgress;
use cigi_source::{
    get_artifact, get_manifest, stage_artifact, DigestTrust, FetchError, NoProgress, Source,
    StubAttest, Trust, TrustLevel,
};
use cigi_testkit::Fixture;
use common::*;
use std::sync::Arc;

fn fixture_with(fault: impl Fn(&mut cigi_testkit::ProductBuilder)) -> Fixture {
    let mut fx = Fixture::new();
    fault(fx.product("cig", "1.0.0"));
    fx.carton("0.1.0", &[("cig", "1.0.0")]);
    fx.publish("stable");
    fx
}

fn fetch_cig(fx: &Fixture, t: &DigestTrust) -> Result<cigi_source::StagedArtifact, FetchError> {
    let src = mirror(fx);
    let m = manifest_of(fx, "stable");
    get_artifact(&src, t, &m, "cig", &v("1.0.0"), &fx.home, &mut NoProgress)
}

#[test]
fn clean_download_is_checksummed_and_not_independent() {
    let fx = fixture_with(|_| {});
    let mut p = CountingProgress::default();
    let src = mirror(&fx);
    let m = manifest_of(&fx, "stable");
    let s = get_artifact(&src, &trust(), &m, "cig", &v("1.0.0"), &fx.home, &mut p).unwrap();
    assert_eq!(s.verified.level, TrustLevel::Checksummed);
    assert!(
        !s.verified.independent,
        "a directory mirror serves manifest and sidecars alike"
    );
    assert_eq!(s.verified.sha256, s.art.sha256);
    assert!(p.finished && p.total.is_some() && p.advanced == s.art.size);
    assert!(s.path.is_file());
    assert!(s.origin.starts_with("dir:"));
}

#[test]
fn corrupt_asset_is_refused_and_nothing_is_kept() {
    let fx = fixture_with(|p| {
        p.corrupt_asset();
    });
    let e = fetch_cig(&fx, &trust()).unwrap_err();
    assert_eq!(e.code(), "E1821", "{e}");
    assert!(
        downloads(&fx).is_empty(),
        "left behind: {:?}",
        downloads(&fx)
    );
}

#[test]
fn truncated_asset_is_a_size_mismatch() {
    let fx = fixture_with(|p| {
        p.truncate();
    });
    let e = fetch_cig(&fx, &trust()).unwrap_err();
    assert_eq!(e.code(), "E1821");
    assert!(e.to_string().contains("bytes"), "{e}");
    assert!(downloads(&fx).is_empty());
}

#[test]
fn bad_sidecar_is_refused() {
    let fx = fixture_with(|p| {
        p.bad_sidecar();
    });
    let e = fetch_cig(&fx, &trust()).unwrap_err();
    assert_eq!(e.code(), "E1827");
    assert!(downloads(&fx).is_empty());
}

#[test]
fn missing_sidecar_is_refused() {
    let fx = fixture_with(|p| {
        p.missing_sidecar();
    });
    let e = fetch_cig(&fx, &trust()).unwrap_err();
    assert_eq!(e.code(), "E1827");
    assert!(downloads(&fx).is_empty());
}

#[test]
fn no_build_for_a_platform() {
    let fx = fixture_with(|_| {});
    let src = mirror(&fx);
    let m = manifest_of(&fx, "stable");
    let other = Target {
        os: "plan9".into(),
        arch: "mips".into(),
    };
    let e = cigi_source::get_artifact_for(
        &src,
        &trust(),
        &m,
        "cig",
        &v("1.0.0"),
        &other,
        &fx.home,
        &mut NoProgress,
    )
    .unwrap_err();
    assert_eq!(e.code(), "E1815");
    let e = get_artifact(
        &src,
        &trust(),
        &m,
        "cigo",
        &v("9.9.9"),
        &fx.home,
        &mut NoProgress,
    )
    .unwrap_err();
    assert_eq!(e.code(), "E1815");
}

static PIN_OK: &[(&str, &str)] = &[];

#[test]
fn floor_pinned_without_a_pin_is_refused_and_a_pin_wins() {
    let fx = fixture_with(|_| {});
    let t = DigestTrust::new(TrustLevel::Pinned)
        .with_pins(PIN_OK)
        .with_attest(Arc::new(cigi_source::NoAttest));
    let e = fetch_cig(&fx, &t).unwrap_err();
    assert_eq!(e.code(), "E1822");
    assert!(downloads(&fx).is_empty());

    // now pin it: leak a one-off static table, as the real one is a const
    let m = manifest_of(&fx, "stable");
    let art = m.artifact("cig", &v("1.0.0"), &Target::host()).unwrap();
    let key = format!("cig-1.0.0-{}", Target::host());
    let table: &'static [(&'static str, &'static str)] = Box::leak(
        vec![(
            Box::leak(key.into_boxed_str()) as &'static str,
            Box::leak(art.sha256.clone().into_boxed_str()) as &'static str,
        )]
        .into_boxed_slice(),
    );
    let t = DigestTrust::new(TrustLevel::Pinned)
        .with_pins(table)
        .with_attest(Arc::new(cigi_source::NoAttest));
    let s = fetch_cig(&fx, &t).unwrap();
    assert_eq!(s.verified.level, TrustLevel::Pinned);
}

#[test]
fn a_pin_that_disagrees_with_the_manifest_is_refused() {
    let fx = fixture_with(|_| {});
    let key = format!("cig-1.0.0-{}", Target::host());
    let table: &'static [(&'static str, &'static str)] = Box::leak(
        vec![(
            Box::leak(key.into_boxed_str()) as &'static str,
            "0000000000000000000000000000000000000000000000000000000000000000",
        )]
        .into_boxed_slice(),
    );
    let t = DigestTrust::new(TrustLevel::Checksummed)
        .with_pins(table)
        .with_attest(Arc::new(cigi_source::NoAttest));
    let e = fetch_cig(&fx, &t).unwrap_err();
    assert_eq!(e.code(), "E1821");
    assert!(e.to_string().contains("pin"));
}

#[test]
fn pinned_needs_no_sidecar() {
    let fx = fixture_with(|p| {
        p.missing_sidecar();
    });
    let m = manifest_of(&fx, "stable");
    let art = m.artifact("cig", &v("1.0.0"), &Target::host()).unwrap();
    let key = format!("cig-1.0.0-{}", Target::host());
    let table: &'static [(&'static str, &'static str)] = Box::leak(
        vec![(
            Box::leak(key.into_boxed_str()) as &'static str,
            Box::leak(art.sha256.clone().into_boxed_str()) as &'static str,
        )]
        .into_boxed_slice(),
    );
    let t = DigestTrust::new(TrustLevel::Checksummed)
        .with_pins(table)
        .with_attest(Arc::new(cigi_source::NoAttest));
    assert_eq!(
        fetch_cig(&fx, &t).unwrap().verified.level,
        TrustLevel::Pinned
    );
}

#[test]
fn attestation_levels_and_failures() {
    let fx = fixture_with(|_| {});
    let src = mirror(&fx);
    let m = manifest_of(&fx, "stable");
    let mut art = m
        .artifact("cig", &v("1.0.0"), &Target::host())
        .unwrap()
        .clone();
    art.attests = true;
    let staged = get_artifact(
        &src,
        &trust(),
        &m,
        "cig",
        &v("1.0.0"),
        &fx.home,
        &mut NoProgress,
    )
    .unwrap();
    let side = std::fs::read_to_string(fx.releases.join(format!("{}.sha256", art.asset))).unwrap();

    let ok = DigestTrust::new(TrustLevel::Attested)
        .with_pins(&[])
        .with_attest(Arc::new(StubAttest(Ok("github:x".into()))));
    let v1 = ok.admit_artifact(&art, Some(&side), &staged.path).unwrap();
    assert_eq!(v1.level, TrustLevel::Attested);
    assert_eq!(v1.attested_by.as_deref(), Some("github:x"));

    let no = DigestTrust::new(TrustLevel::Attested)
        .with_pins(&[])
        .with_attest(Arc::new(StubAttest(Err("bad signature".into()))));
    assert_eq!(
        no.admit_artifact(&art, Some(&side), &staged.path)
            .unwrap_err()
            .code(),
        "E1828"
    );

    // floor checksummed: a failed attestation is not fatal, the level stays checksummed
    let soft = DigestTrust::new(TrustLevel::Checksummed)
        .with_pins(&[])
        .with_attest(Arc::new(StubAttest(Err("bad".into()))));
    assert_eq!(
        soft.admit_artifact(&art, Some(&side), &staged.path)
            .unwrap()
            .level,
        TrustLevel::Checksummed
    );

    // floor attested, tool does not attest: E1822
    art.attests = false;
    assert_eq!(
        ok.admit_artifact(&art, Some(&side), &staged.path)
            .unwrap_err()
            .code(),
        "E1822"
    );
    // gh missing entirely
    art.attests = true;
    let none = DigestTrust::new(TrustLevel::Attested)
        .with_pins(&[])
        .with_attest(Arc::new(cigi_source::NoAttest));
    assert_eq!(
        none.admit_artifact(&art, Some(&side), &staged.path)
            .unwrap_err()
            .code(),
        "E1822"
    );
}

#[test]
fn sidecar_that_is_not_a_digest_is_refused() {
    let fx = fixture_with(|_| {});
    let m = manifest_of(&fx, "stable");
    let art = m.artifact("cig", &v("1.0.0"), &Target::host()).unwrap();
    let staged = fetch_cig(&fx, &trust()).unwrap();
    let e = trust()
        .admit_artifact(art, Some("<html>502 Bad Gateway</html>"), &staged.path)
        .unwrap_err();
    assert_eq!(e.code(), "E1825");
}

#[test]
fn level_is_digest() {
    assert_eq!(trust().level(), "digest");
}

// ---- staging

#[test]
fn binary_layout_stages_one_executable() {
    let fx = fixture_with(|_| {});
    let m = manifest_of(&fx, "stable");
    let s = fetch_cig(&fx, &trust()).unwrap();
    let stage = fx.tmp.path().join("stage");
    let files = stage_artifact(&m, &s, &stage).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path.to_string_lossy(), "bin/cig");
    assert!(files[0].exec);
    assert_eq!(
        cigi_testkit::sha256_hex(&std::fs::read(&files[0].source).unwrap()),
        s.art.sha256
    );
    assert!(stage.join("bin/cig").is_file());
}

#[test]
fn archive_layout_strips_and_lists_sorted() {
    let mut fx = Fixture::new();
    fx.carton("0.1.0", &[("cigo", "1.17.0")]);
    fx.publish("stable");
    let src = mirror(&fx);
    let m = manifest_of(&fx, "stable");
    let s = get_artifact(
        &src,
        &trust(),
        &m,
        "cigo",
        &v("1.17.0"),
        &fx.home,
        &mut NoProgress,
    )
    .unwrap();
    assert_eq!(s.art.strip, 1);
    let stage = fx.tmp.path().join("stage");
    let files = stage_artifact(&m, &s, &stage).unwrap();
    let names: Vec<String> = files
        .iter()
        .map(|f| f.path.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        ["LICENSE", "bin/cigo", "docs/a.md", "orchestrate.cig"]
    );
    assert!(
        files
            .iter()
            .find(|f| f.path.ends_with("cigo"))
            .unwrap()
            .exec
    );
    assert!(
        !files
            .iter()
            .find(|f| f.path.ends_with("LICENSE"))
            .unwrap()
            .exec
    );
}

#[test]
fn zip_slip_passes_every_digest_and_is_stopped_at_unpack() {
    let mut fx = Fixture::new();
    fx.product("cigo", "1.17.0").zip_slip();
    fx.carton("0.1.0", &[("cigo", "1.17.0")]);
    fx.publish("stable");
    let src = mirror(&fx);
    let m = manifest_of(&fx, "stable");
    let s = get_artifact(
        &src,
        &trust(),
        &m,
        "cigo",
        &v("1.17.0"),
        &fx.home,
        &mut NoProgress,
    )
    .unwrap();
    let stage = fx.tmp.path().join("stage");
    let e = stage_artifact(&m, &s, &stage).unwrap_err();
    assert_eq!(e.code(), "E1826", "{e}");
    assert!(!fx.tmp.path().join("escape.txt").exists());
}

#[test]
fn dedup_friendly_common_files_have_equal_digests() {
    let mut fx = Fixture::new();
    fx.carton("0.1.0", &[("cigo", "1.16.0")]);
    fx.carton("0.2.0", &[("cigo", "1.17.0")]);
    fx.publish("stable");
    let src = mirror(&fx);
    let m = manifest_of(&fx, "stable");
    let mut sets = Vec::new();
    for ver in ["1.16.0", "1.17.0"] {
        let s = get_artifact(
            &src,
            &trust(),
            &m,
            "cigo",
            &v(ver),
            &fx.home,
            &mut NoProgress,
        )
        .unwrap();
        sets.push(stage_artifact(&m, &s, &fx.tmp.path().join(format!("s{ver}"))).unwrap());
    }
    let sha = |set: &Vec<cigi_home::StagedFile>, n: &str| {
        let f = set.iter().find(|f| f.path.ends_with(n)).unwrap();
        cigi_testkit::sha256_hex(&std::fs::read(&f.source).unwrap())
    };
    assert_eq!(sha(&sets[0], "LICENSE"), sha(&sets[1], "LICENSE"));
    assert_ne!(
        sha(&sets[0], "orchestrate.cig"),
        sha(&sets[1], "orchestrate.cig")
    );
    assert_ne!(sha(&sets[0], "cigo"), sha(&sets[1], "cigo"));
}

// ---- the manifest and its cache

#[test]
fn manifest_round_trip_and_cache() {
    let mut fx = Fixture::new();
    fx.carton("0.1.0", &[("cig", "1.0.0")]);
    fx.publish("stable");
    let src = mirror(&fx);
    let m = get_manifest(&src, &trust(), &fx.home, "stable", false).unwrap();
    assert_eq!(m.sequence, 1);
    assert_eq!(m.build_in(&v("0.1.0"), "cig"), Some(&v("1.0.0")));
    let dir = fx.cigscript().join("installer/cache/manifests/stable");
    for f in ["manifest.toml", "manifest.toml.sha256", "fetched.json"] {
        assert!(dir.join(f).is_file(), "{f}");
    }
    // offline reads the cache; the source is not asked
    let off = get_manifest(&DeadSource, &trust(), &fx.home, "stable", true).unwrap();
    assert_eq!(off.sequence, 1);
    assert!(cigi_source::fetch::cache_age_secs(&fx.home, "stable", None).unwrap() < 60);
}

#[test]
fn offline_with_nothing_cached_is_e1810() {
    let fx = Fixture::new();
    let e = get_manifest(&mirror(&fx), &trust(), &fx.home, "stable", true).unwrap_err();
    assert_eq!(e.code(), "E1810");
}

#[test]
fn missing_manifest_online_is_a_download_failure() {
    let fx = Fixture::new();
    let e = get_manifest(&mirror(&fx), &trust(), &fx.home, "stable", false).unwrap_err();
    assert_eq!(e.code(), "E1820");
}

#[test]
fn sequence_going_backwards_is_refused_and_the_cache_keeps_the_newer() {
    let mut fx = Fixture::new();
    fx.carton("0.1.0", &[("cig", "1.0.0")]);
    fx.publish_with(
        "stable",
        &cigi_testkit::PublishOpts {
            sequence: Some(5),
            ..Default::default()
        },
    );
    let src = mirror(&fx);
    assert_eq!(
        get_manifest(&src, &trust(), &fx.home, "stable", false)
            .unwrap()
            .sequence,
        5
    );
    // equal is fine
    assert_eq!(
        get_manifest(&src, &trust(), &fx.home, "stable", false)
            .unwrap()
            .sequence,
        5
    );
    fx.publish_with(
        "stable",
        &cigi_testkit::PublishOpts {
            sequence: Some(3),
            ..Default::default()
        },
    );
    let e = get_manifest(&src, &trust(), &fx.home, "stable", false).unwrap_err();
    assert_eq!(e.code(), "E1824");
    let cached = cigi_source::fetch::read_fetched(&fx.home, "stable").unwrap();
    assert_eq!(cached.sequence, 5);
    // and higher moves on
    fx.publish_with(
        "stable",
        &cigi_testkit::PublishOpts {
            sequence: Some(6),
            ..Default::default()
        },
    );
    assert_eq!(
        get_manifest(&src, &trust(), &fx.home, "stable", false)
            .unwrap()
            .sequence,
        6
    );
}

#[test]
fn expired_manifest_is_refused_unless_offline() {
    let mut fx = Fixture::new();
    fx.carton("0.1.0", &[("cig", "1.0.0")]);
    fx.publish_with(
        "stable",
        &cigi_testkit::PublishOpts {
            expires_in_days: Some(-1),
            ..Default::default()
        },
    );
    let src = mirror(&fx);
    let e = get_manifest(&src, &trust(), &fx.home, "stable", false).unwrap_err();
    assert_eq!(e.code(), "E1823");
    assert!(
        cigi_source::fetch::read_fetched(&fx.home, "stable").is_none(),
        "an expired manifest must not be cached"
    );
    // a fresh one cached, then time moves on: offline still works, with a note
    fx.publish("stable");
    get_manifest(&src, &trust(), &fx.home, "stable", false).unwrap();
    let opts = cigi_source::ManifestOpts {
        offline: true,
        now: Some(cigi_source::clock::now() + 90 * 86400),
        ..Default::default()
    };
    let m = cigi_source::get_manifest_with(&src, &trust(), &fx.home, "stable", &opts).unwrap();
    assert!(
        m.notes().iter().any(|n| n.contains("expired")),
        "{:?}",
        m.notes()
    );
    let strict = cigi_source::ManifestOpts {
        offline: false,
        now: Some(cigi_source::clock::now() + 90 * 86400),
        ..Default::default()
    };
    assert_eq!(
        cigi_source::get_manifest_with(&src, &trust(), &fx.home, "stable", &strict)
            .unwrap_err()
            .code(),
        "E1823"
    );
}

#[test]
fn manifest_sidecar_rules() {
    let mut fx = Fixture::new();
    fx.carton("0.1.0", &[("cig", "1.0.0")]);
    let src = mirror(&fx);
    fx.publish_with(
        "stable",
        &cigi_testkit::PublishOpts {
            bad_sidecar: true,
            ..Default::default()
        },
    );
    assert_eq!(
        get_manifest(&src, &trust(), &fx.home, "stable", false)
            .unwrap_err()
            .code(),
        "E1827"
    );
    fx.publish_with(
        "stable",
        &cigi_testkit::PublishOpts {
            no_sidecar: true,
            ..Default::default()
        },
    );
    assert_eq!(
        get_manifest(&src, &trust(), &fx.home, "stable", false)
            .unwrap_err()
            .code(),
        "E1827"
    );
    assert!(cigi_source::fetch::read_fetched(&fx.home, "stable").is_none());
}

#[test]
fn malformed_manifest_and_wrong_channel() {
    let mut fx = Fixture::new();
    fx.carton("0.1.0", &[("cig", "1.0.0")]);
    fx.publish("beta");
    let src = mirror(&fx);
    // the beta manifest copied to the stable name, sidecar recomputed: still refused, it says "beta"
    let text = std::fs::read_to_string(fx.releases.join("manifest-beta.toml")).unwrap();
    std::fs::write(fx.releases.join("manifest-stable.toml"), &text).unwrap();
    std::fs::write(
        fx.releases.join("manifest-stable.toml.sha256"),
        format!("{}  x\n", cigi_testkit::sha256_hex(text.as_bytes())),
    )
    .unwrap();
    assert_eq!(
        get_manifest(&src, &trust(), &fx.home, "stable", false)
            .unwrap_err()
            .code(),
        "E1825"
    );
    // garbage with a matching sidecar
    let junk = "this is = not [valid";
    std::fs::write(fx.releases.join("manifest-stable.toml"), junk).unwrap();
    std::fs::write(
        fx.releases.join("manifest-stable.toml.sha256"),
        format!("{}  x\n", cigi_testkit::sha256_hex(junk.as_bytes())),
    )
    .unwrap();
    assert_eq!(
        get_manifest(&src, &trust(), &fx.home, "stable", false)
            .unwrap_err()
            .code(),
        "E1825"
    );
    // a channel name that is a path
    assert_eq!(
        get_manifest(&src, &trust(), &fx.home, "../x", false)
            .unwrap_err()
            .code(),
        "E1820"
    );
}

#[test]
fn trust_is_independent_of_the_source() {
    // the trait object works for any Source
    let fx = Fixture::new();
    let s: Box<dyn Source> = Box::new(mirror(&fx));
    assert!(!s.independent());
    assert!(cigi_source::GithubSource.independent());
}
