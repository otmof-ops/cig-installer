// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0
//! The HTTP client against the loopback fixture server: only loopback, never the internet.

mod common;
use cigi_source::http::{self, HttpConfig};
use cigi_source::progress::CountingProgress;
use cigi_source::{
    get_artifact, get_manifest, FetchError, Location, ManifestPart, MirrorSource, NoProgress,
    Source,
};
use cigi_testkit::{Fixture, Rule};
use common::*;
use std::time::Duration;
use url::Url;

fn served() -> (Fixture, cigi_testkit::FixtureServer, MirrorSource) {
    let mut fx = Fixture::new();
    fx.carton("0.1.0", &[("cig", "1.0.0"), ("cigo", "1.0.0")]);
    fx.publish("stable");
    let srv = fx.serve();
    let m = MirrorSource::parse(&srv.url()).unwrap();
    (fx, srv, m)
}

fn manifest_loc() -> Location {
    Location::Manifest {
        channel: "stable".into(),
        part: ManifestPart::Doc,
    }
}

#[test]
fn install_flow_requests_obey_the_no_telemetry_rules() {
    let (fx, srv, m) = served();
    let man = get_manifest(&m, &trust(), &fx.home, "stable", false).unwrap();
    for (t, ver) in [("cig", "1.0.0"), ("cigo", "1.0.0")] {
        get_artifact(&m, &trust(), &man, t, &v(ver), &fx.home, &mut NoProgress).unwrap();
    }
    let reqs = srv.requests();
    assert!(reqs.len() >= 7, "{reqs:?}");
    srv.assert_no_telemetry().unwrap();
    for r in &reqs {
        assert_eq!(r.method, "GET");
        assert_eq!(r.header("user-agent"), Some("ciginstaller"));
        assert!(r.query.is_none());
        assert!(r.header("cookie").is_none() && r.header("authorization").is_none());
    }
    // each product: sidecar then asset, nothing else; the manifest: doc, sidecar (attestation only with gh)
    let paths = srv.paths();
    assert!(paths.iter().any(|p| p == "/manifest-stable.toml"));
    assert!(paths.iter().any(|p| p.ends_with(".sha256")));
}

#[test]
fn manifest_and_asset_over_loopback_http() {
    let (fx, srv, m) = served();
    assert!(!m.independent());
    assert!(m.name().starts_with("mirror:http://127.0.0.1:"));
    let txt = m.fetch_text(&manifest_loc(), 1 << 20).unwrap();
    assert!(txt.contains("channel   = \"stable\""));
    let mut p = CountingProgress::default();
    let dest = fx.tmp.path().join("dl");
    let info = m
        .fetch(
            &Location::Asset {
                repo: "otmof-ops/cig-script".into(),
                tag: "v1.0.0".into(),
                name: cigi_testkit::asset_name("cig", "1.0.0"),
            },
            &dest,
            &mut p,
        )
        .unwrap();
    assert_eq!(info.bytes, std::fs::metadata(&dest).unwrap().len());
    assert_eq!(
        info.sha256,
        cigi_testkit::sha256_hex(&std::fs::read(&dest).unwrap())
    );
    assert_eq!(p.total, Some(info.bytes));
    assert!(p.finished);
    drop(srv);
}

#[test]
fn http_404_is_not_found_and_other_statuses_are_network_errors() {
    let (_fx, srv, m) = served();
    let e = m
        .fetch_text(
            &Location::Asset {
                repo: "a/b".into(),
                tag: "v1".into(),
                name: "nothing.here".into(),
            },
            100,
        )
        .unwrap_err();
    assert!(e.is_not_found(), "{e}");
    assert_eq!(e.code(), "E1820");
    srv.rule("/manifest-stable.toml", Rule::Status(503));
    let e = m.fetch_text(&manifest_loc(), 1 << 20).unwrap_err();
    assert!(
        matches!(e, FetchError::Network { .. }) && !e.is_not_found(),
        "{e}"
    );
}

#[test]
fn redirect_to_the_same_host_is_followed() {
    let (_fx, srv, m) = served();
    srv.rule(
        "/manifest-stable.toml",
        Rule::Redirect(format!("{}/manifest-beta-alias.toml", srv.url())),
    );
    // the target does not exist: a 404 after following proves the hop happened
    let e = m.fetch_text(&manifest_loc(), 1 << 20).unwrap_err();
    assert!(e.is_not_found(), "{e}");
    assert!(srv
        .paths()
        .contains(&"/manifest-beta-alias.toml".to_string()));
    // and to an existing file: served
    srv.rule(
        "/manifest-stable.toml",
        Rule::Redirect(format!("{}/manifest-stable.toml.sha256", srv.url())),
    );
    assert!(m
        .fetch_text(&manifest_loc(), 1 << 20)
        .unwrap()
        .contains("manifest-stable.toml"));
}

#[test]
fn redirect_to_another_host_is_refused() {
    let (_fx, srv, m) = served();
    for target in [
        "http://localhost:1/x",
        "http://127.0.0.1:1/x",
        "https://example.com/x",
    ] {
        srv.rule("/manifest-stable.toml", Rule::Redirect(target.into()));
        let e = m.fetch_text(&manifest_loc(), 1 << 20).unwrap_err();
        assert!(matches!(e, FetchError::Refused { .. }), "{target}: {e}");
        assert_eq!(e.code(), "E1820");
    }
}

#[test]
fn redirect_loops_stop() {
    let (_fx, srv, m) = served();
    srv.rule(
        "/manifest-stable.toml",
        Rule::Redirect(format!("{}/manifest-stable.toml", srv.url())),
    );
    let e = m.fetch_text(&manifest_loc(), 1 << 20).unwrap_err();
    assert!(e.to_string().contains("too many redirects"), "{e}");
}

#[test]
fn github_policy_allows_the_release_cdn_only() {
    // exercised through the policy on the loopback: SameHost refuses what Github would refuse too,
    // and the allow-list itself is checked on URLs
    let (_fx, srv, _m) = served();
    let cfg = HttpConfig {
        redirects: http::RedirectPolicy::Github,
        ..HttpConfig::default()
    };
    srv.rule(
        "/manifest-stable.toml",
        Rule::Redirect("http://example.com/x".into()),
    );
    let u = Url::parse(&format!("{}/manifest-stable.toml", srv.url())).unwrap();
    let e = http::get_text(&u, &cfg, 1000).unwrap_err();
    assert!(matches!(e, FetchError::Refused { .. }), "{e}");
}

#[test]
fn truncated_body_is_a_download_failure() {
    let (fx, _srv, m) = served();
    let man = get_manifest(&m, &trust(), &fx.home, "stable", false).unwrap();
    // an asset server that claims 5000 bytes, sends 100 and hangs up
    let short = cigi_testkit::ShortBodyServer::start(vec![7u8; 100], 5000);
    let liar = MirrorSource::parse(&short.url()).unwrap();
    let e = get_artifact(
        &liar,
        &trust(),
        &man,
        "cigo",
        &v("1.0.0"),
        &fx.home,
        &mut NoProgress,
    )
    .unwrap_err();
    assert_eq!(e.code(), "E1820", "{e}");
    assert!(
        e.to_string().contains("stopped early") || e.to_string().contains("ended after"),
        "{e}"
    );
    assert!(downloads(&fx).is_empty(), "{:?}", downloads(&fx));
}

#[test]
fn a_stalled_body_times_out() {
    let (fx, srv, _m) = served();
    let name = cigi_testkit::asset_name("cigo", "1.0.0");
    srv.rule(&format!("/{name}"), Rule::Stall(Duration::from_secs(3)));
    let cfg = HttpConfig {
        read_timeout: Duration::from_millis(300),
        ..HttpConfig::default()
    };
    let u = Url::parse(&format!("{}/{name}", srv.url())).unwrap();
    let t0 = std::time::Instant::now();
    let e = http::get_to_file(&u, &cfg, &fx.tmp.path().join("slow"), &mut NoProgress).unwrap_err();
    assert!(matches!(e, FetchError::Network { .. }), "{e}");
    assert!(
        t0.elapsed() < Duration::from_secs(2),
        "took {:?}",
        t0.elapsed()
    );
}

#[test]
fn size_limit_is_enforced() {
    let (fx, srv, _m) = served();
    let u = Url::parse(&format!("{}/manifest-stable.toml", srv.url())).unwrap();
    let cfg = HttpConfig {
        max_bytes: 100,
        ..HttpConfig::default()
    };
    assert!(matches!(
        http::get_to_file(&u, &cfg, &fx.tmp.path().join("x"), &mut NoProgress).unwrap_err(),
        FetchError::TooLarge { .. }
    ));
    assert!(matches!(
        http::get_text(&u, &cfg, 10).unwrap_err(),
        FetchError::TooLarge { .. }
    ));
}

#[test]
fn plain_http_is_loopback_only() {
    for bad in [
        "http://example.com/",
        "http://192.168.1.5/",
        "http://localhost/",
        "ftp://example.com/",
        "https://u:p@example.com/",
        "https://example.com/?a=b",
    ] {
        assert!(
            MirrorSource::new(Url::parse(bad).unwrap()).is_err(),
            "{bad}"
        );
    }
    for ok in [
        "http://127.0.0.1:8080/",
        "http://127.7.7.7/",
        "http://[::1]:9/",
        "https://example.com/rel/",
        "file:///tmp/x/",
    ] {
        assert!(MirrorSource::new(Url::parse(ok).unwrap()).is_ok(), "{ok}");
    }
    // a hand-built MirrorSource{base} is checked again at fetch time
    let m = MirrorSource {
        base: Url::parse("http://example.com/").unwrap(),
    };
    assert!(matches!(
        m.fetch_text(&manifest_loc(), 10).unwrap_err(),
        FetchError::Refused { .. }
    ));
}

#[test]
fn a_slow_drip_hits_the_overall_deadline() {
    let (fx, srv, _m) = served();
    let name = cigi_testkit::asset_name("cigo", "1.0.0");
    srv.rule(&format!("/{name}"), Rule::Stall(Duration::from_millis(600)));
    let cfg = HttpConfig {
        total_timeout: Duration::from_millis(100),
        ..HttpConfig::default()
    };
    let u = Url::parse(&format!("{}/{name}", srv.url())).unwrap();
    let e = http::get_to_file(&u, &cfg, &fx.tmp.path().join("slow2"), &mut NoProgress).unwrap_err();
    assert!(e.to_string().contains("took longer"), "{e}");
}

#[test]
fn names_that_could_climb_are_refused() {
    let (_fx, _srv, m) = served();
    for name in ["../etc/passwd", "a/b", ".hidden", "", "a b", "x\\y"] {
        let e = m
            .fetch_text(
                &Location::Asset {
                    repo: "a/b".into(),
                    tag: "v1".into(),
                    name: name.into(),
                },
                10,
            )
            .unwrap_err();
        assert!(matches!(e, FetchError::Refused { .. }), "{name:?}: {e}");
    }
    assert!(cigi_source::GithubSource::url_for(&Location::Asset {
        repo: "a/../b".into(),
        tag: "v1".into(),
        name: "x".into()
    })
    .is_err());
    assert!(cigi_source::GithubSource::url_for(&Location::Asset {
        repo: "otmof-ops/cig-script".into(),
        tag: "v1.0.0".into(),
        name: "a?b=1".into()
    })
    .is_err());
}

#[test]
fn plain_http_only_to_literal_loopback_addresses() {
    use cigi_source::http::check_scheme;
    use url::Url;
    for ok in [
        "http://127.0.0.1/x",
        "http://127.9.9.9:80/x",
        "http://[::1]/x",
        "https://example.com/x",
    ] {
        assert!(check_scheme(&Url::parse(ok).unwrap()).is_ok(), "{ok}");
    }
    for bad in [
        "http://localhost/x",
        "http://LOCALHOST:8080/x",
        "http://example.com/x",
        "http://[::2]/x",
        "file:///etc/passwd",
    ] {
        assert!(check_scheme(&Url::parse(bad).unwrap()).is_err(), "{bad}");
    }
}

#[test]
fn only_release_asset_hosts_are_github_hosts() {
    use cigi_source::http::github_host;
    use url::Url;
    for ok in [
        "https://github.com/a/b",
        "https://objects.githubusercontent.com/x",
        "https://release-assets.githubusercontent.com/x",
        "https://github-releases.githubusercontent.com/x",
    ] {
        assert!(github_host(&Url::parse(ok).unwrap()), "{ok}");
    }
    for bad in [
        "https://raw.githubusercontent.com/a/b/c",
        "https://gist.githubusercontent.com/x",
        "https://evil.githubusercontent.com/x",
        "https://github.com.evil.example/x",
        "http://github.com/x",
        "https://github.com:8443/x",
    ] {
        assert!(!github_host(&Url::parse(bad).unwrap()), "{bad}");
    }
}
