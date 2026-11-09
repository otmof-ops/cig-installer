// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0
//! Regression tests from the SEC-004 review of PR #36: manifest attestation, attestation repos,
//! single-source mirrors, the `expires` horizon, manifest URLs and partial files.

mod common;
use cig_compat::Target;
use cigi_source::{
    get_artifact, get_manifest_with, DigestTrust, GithubSource, Location, ManifestOpts, NoAttest,
    NoProgress, SeenState, StubAttest, Trust, TrustLevel,
};
use cigi_testkit::{Fixture, PublishOpts};
use common::*;
use std::sync::Arc;

fn fx_days(days: i64) -> Fixture {
    let mut fx = Fixture::new();
    fx.product("cig", "1.0.0");
    fx.carton("0.1.0", &[("cig", "1.0.0")]);
    fx.publish_with(
        "stable",
        &PublishOpts {
            expires_in_days: Some(days),
            ..PublishOpts::default()
        },
    );
    fx
}

fn doc_and_sidecar(fx: &Fixture) -> (Vec<u8>, String) {
    (
        std::fs::read(fx.releases.join("manifest-stable.toml")).unwrap(),
        std::fs::read_to_string(fx.releases.join("manifest-stable.toml.sha256")).unwrap(),
    )
}

fn seen() -> SeenState {
    SeenState {
        channel: Some("stable".into()),
        ..SeenState::default()
    }
}

#[test]
fn single_source_mirror_does_not_reach_checksummed_by_default() {
    let fx = fx_days(30);
    let src = mirror(&fx);
    let m = manifest_of(&fx, "stable");
    let e = get_artifact(
        &src,
        &strict_trust(),
        &m,
        "cig",
        &v("1.0.0"),
        &fx.home,
        &mut NoProgress,
    )
    .unwrap_err();
    assert_eq!(e.code(), "E1822", "{e}");
    assert!(e.to_string().contains("allow_single_source"), "{e}");
    assert!(downloads(&fx).is_empty(), "{:?}", downloads(&fx));
    // with the opt-in it is accepted
    assert!(get_artifact(
        &src,
        &strict_trust().with_single_source(true),
        &m,
        "cig",
        &v("1.0.0"),
        &fx.home,
        &mut NoProgress
    )
    .is_ok());
}

#[test]
fn manifest_without_a_bundle_is_refused_from_attested_up_and_noted_below() {
    let fx = fx_days(30);
    let (doc, side) = doc_and_sidecar(&fx);
    let strict = DigestTrust::new(TrustLevel::Attested)
        .with_pins(&[])
        .with_attest(Arc::new(StubAttest(Ok("github:x".into()))));
    let e = strict
        .admit_manifest(&doc, Some(&side), None, &seen())
        .unwrap_err();
    assert_eq!(e.code(), "E1828", "{e}");
    // gh missing, bundle present
    let nogh = DigestTrust::new(TrustLevel::Attested)
        .with_pins(&[])
        .with_attest(Arc::new(NoAttest));
    let e = nogh
        .admit_manifest(&doc, Some(&side), Some(b"{}"), &seen())
        .unwrap_err();
    assert_eq!(e.code(), "E1828", "{e}");
    // a bundle that does not verify
    let bad = DigestTrust::new(TrustLevel::Attested)
        .with_pins(&[])
        .with_attest(Arc::new(StubAttest(Err("nope".into()))));
    assert_eq!(
        bad.admit_manifest(&doc, Some(&side), Some(b"{}"), &seen())
            .unwrap_err()
            .code(),
        "E1828"
    );
    // a good bundle passes
    let ok = strict
        .admit_manifest(&doc, Some(&side), Some(b"{}"), &seen())
        .unwrap();
    assert!(ok
        .notes()
        .iter()
        .any(|n| n.contains("attestation verified")));
    // from the cache (offline) the bundle is not kept: not refused again
    let offline = SeenState {
        offline: true,
        ..seen()
    };
    assert!(strict
        .admit_manifest(&doc, Some(&side), None, &offline)
        .is_ok());
    // below attested: admitted, with the note
    let soft = DigestTrust::new(TrustLevel::Checksummed)
        .with_pins(&[])
        .with_attest(Arc::new(NoAttest));
    let a = soft
        .admit_manifest(&doc, Some(&side), None, &seen())
        .unwrap();
    assert!(
        a.notes().iter().any(|n| n == "manifest not attested"),
        "{:?}",
        a.notes()
    );
}

#[test]
fn attestation_repo_must_be_on_the_allow_list() {
    let fx = fx_days(30);
    let m = manifest_of(&fx, "stable");
    let src = mirror(&fx);
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
    let mut art = m
        .artifact("cig", &v("1.0.0"), &Target::host())
        .unwrap()
        .clone();
    let side = std::fs::read_to_string(fx.releases.join(format!("{}.sha256", art.asset))).unwrap();
    art.attests = true;
    let attested = DigestTrust::new(TrustLevel::Attested)
        .with_pins(&[])
        .with_attest(Arc::new(StubAttest(Ok("github:whoever".into()))));
    for repo in [
        "attacker/evil",
        "otmof-ops/evil",
        "otmof-ops/cig-script-fork",
    ] {
        art.repo = repo.into();
        let e = attested
            .admit_artifact(&art, Some(&side), &staged.path)
            .unwrap_err();
        assert_eq!(e.code(), "E1828", "{repo}: {e}");
    }
    for repo in [
        "otmof-ops/cig-script",
        "otmof-ops/cig-orchestrator",
        "otmof-ops/cig-compile",
        "otmof-ops/cigar-script",
    ] {
        art.repo = repo.into();
        assert_eq!(
            attested
                .admit_artifact(&art, Some(&side), &staged.path)
                .unwrap()
                .level,
            TrustLevel::Attested,
            "{repo}"
        );
    }
    // below attested a disallowed repo is just not attested
    art.repo = "attacker/evil".into();
    let soft = DigestTrust::new(TrustLevel::Checksummed)
        .with_pins(&[])
        .with_attest(Arc::new(StubAttest(Ok("github:x".into()))));
    assert_eq!(
        soft.admit_artifact(&art, Some(&side), &staged.path)
            .unwrap()
            .level,
        TrustLevel::Checksummed
    );
}

#[test]
fn expires_more_than_90_days_ahead_is_refused_online_only() {
    let far = fx_days(120);
    let (doc, side) = doc_and_sidecar(&far);
    let t = strict_trust();
    let e = t
        .admit_manifest(&doc, Some(&side), None, &seen())
        .unwrap_err();
    assert_eq!(e.code(), "E1825", "{e}");
    assert!(e.to_string().contains("90 days"), "{e}");
    let offline = SeenState {
        offline: true,
        ..seen()
    };
    assert!(t.admit_manifest(&doc, Some(&side), None, &offline).is_ok());
    // through the fetch path nothing is cached after a refusal
    let src = mirror(&far);
    let opts = ManifestOpts::default();
    assert!(get_manifest_with(&src, &t, &far.home, "stable", &opts).is_err());
    assert!(!far
        .cigscript()
        .join("installer/cache/manifests/stable/manifest.toml")
        .exists());
    // 89 days is fine
    let near = fx_days(89);
    let (doc, side) = doc_and_sidecar(&near);
    assert!(t.admit_manifest(&doc, Some(&side), None, &seen()).is_ok());
}

#[test]
fn manifest_url_may_not_leave_github_release_hosts() {
    let at = |u: &str| GithubSource::url_for(&Location::Url { url: u.into() });
    assert!(at("https://github.com/otmof-ops/cig-script/releases/download/v1/cig").is_ok());
    assert!(at("https://objects.githubusercontent.com/x/cig").is_ok());
    for bad in [
        "https://evil.example/cig",
        "https://raw.githubusercontent.com/a/b/c/cig",
        "https://github.com.evil.example/cig",
        "http://github.com/cig",
        "https://github.com:8443/cig",
    ] {
        let e = at(bad).unwrap_err();
        assert_eq!(e.code(), "E1820", "{bad}");
    }
}

#[test]
fn random_suffixes_differ_and_private_files_are_exclusive() {
    let a = cigi_source::digest::random_suffix();
    let b = cigi_source::digest::random_suffix();
    assert_eq!(a.len(), 32);
    assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
    assert_ne!(a, b);
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("f");
    let _f = cigi_source::http::create_private(&p).unwrap();
    assert!(
        cigi_source::http::create_private(&p).is_err(),
        "must not reuse a file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::{symlink, PermissionsExt};
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let target = d.path().join("victim");
        std::fs::write(&target, "keep").unwrap();
        let l = d.path().join("planted");
        symlink(&target, &l).unwrap();
        assert!(
            cigi_source::http::create_private(&l).is_err(),
            "must not follow a planted link"
        );
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep");
    }
}

#[test]
fn partial_download_names_are_not_the_pid() {
    // a download that fails leaves nothing; and a file planted at the old pid-based name is untouched
    let fx = fx_days(30);
    let dl = fx.cigscript().join("installer/cache/downloads");
    std::fs::create_dir_all(&dl).unwrap();
    let m = manifest_of(&fx, "stable");
    let art = m.artifact("cig", &v("1.0.0"), &Target::host()).unwrap();
    let planted = dl.join(format!("{}.partial-{}", art.asset, std::process::id()));
    std::fs::write(&planted, "planted").unwrap();
    let src = mirror(&fx);
    get_artifact(
        &src,
        &trust(),
        &m,
        "cig",
        &v("1.0.0"),
        &fx.home,
        &mut NoProgress,
    )
    .unwrap();
    assert_eq!(std::fs::read_to_string(&planted).unwrap(), "planted");
}
