// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! Mirrors and the transport (#48). A plain `http://` mirror is refused when it is set and found
//! by `doctor`. On loopback it is accepted for pinned builds only, so `allow_single_source` does
//! not open it. An `https://` mirror is checked against the system's trust store. A redirect
//! from https to http is refused. The fixture's HTTPS server uses a throwaway certificate, which
//! the installer trusts only when it is handed over as `SSL_CERT_FILE`, the way OpenSSL tools
//! take one.
#![cfg(unix)]

mod common;

use cigi_testkit::{Fixture, Rule};
use common::{cigi, fixture, ok, text};

fn two(fx: &mut Fixture) {
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.16.0")]);
    fx.carton("0.2.0", &[("cig", "1.21.0"), ("cigo", "1.17.0")]);
    fx.publish("stable");
}

fn config_file(fx: &Fixture) -> String {
    std::fs::read_to_string(fx.cigscript().join("installer/config")).unwrap_or_default()
}

#[test]
fn http_mirror_refused_at_config_set() {
    let fx = fixture();
    let o = cigi(&fx, &["config", "set", "mirror", "http://mirror.example/m"]);
    assert_ne!(o.status.code(), Some(0), "{}", text(&o));
    assert!(text(&o).contains("E1900"), "{}", text(&o));
    assert!(
        !config_file(&fx).contains("mirror.example"),
        "nothing is written"
    );
    // loopback is for testing, and https and a local folder are always fine
    for good in [
        "http://127.0.0.1:8765/m",
        "https://mirror.example/m",
        "file:///srv/mirror",
    ] {
        ok(&cigi(&fx, &["config", "set", "mirror", good]));
        assert!(config_file(&fx).contains(good), "{good}");
    }
}

#[test]
fn doctor_reports_an_http_mirror() {
    let mut fx = fixture();
    two(&mut fx);
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    // written by hand, past `config set`
    let path = fx.cigscript().join("installer/config");
    let mut t = config_file(&fx);
    t.push_str("mirror = http://mirror.example/m\n");
    std::fs::write(&path, t).unwrap();
    let o = fx
        .cigi()
        .env_remove("CIGI_MIRROR")
        .env("CIGI_ALLOW_SINGLE_SOURCE", "true")
        .args(["--json", "doctor"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2), "an action finding: {}", text(&o));
    let t = text(&o);
    assert!(t.contains("http://mirror.example/m is plain http"), "{t}");
    assert!(t.contains("ciginstaller config unset mirror"), "{t}");
}

#[test]
fn loopback_http_refuses_a_build_no_pin_vouches_for() {
    let mut fx = fixture();
    two(&mut fx);
    let plain = fx.serve();
    let o = fx
        .cigi_with_server(&plain)
        .env("CIGI_ALLOW_SINGLE_SOURCE", "true")
        .args(["--yes", "install", "carton", "--carton", "0.1.0"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(text(&o).contains("E1900"), "{}", text(&o));
    assert!(
        !plain.paths().iter().any(|p| p.contains("-v1.")),
        "refused before any build was asked for: {:?}",
        plain.paths()
    );
    // the same builds over https, single source and all, install
    let tls = fx.serve_tls();
    let o = fx
        .cigi_with_server(&tls)
        .env("CIGI_ALLOW_SINGLE_SOURCE", "true")
        .args(["--yes", "install", "carton", "--carton", "0.1.0"])
        .output()
        .unwrap();
    ok(&o);
    assert!(
        tls.paths().iter().any(|p| p.contains("cig-v1.20.0")),
        "{:?}",
        tls.paths()
    );
}

#[test]
fn https_to_http_redirect_refused() {
    let mut fx = fixture();
    two(&mut fx);
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    let plain = fx.serve();
    let tls = fx.serve_tls();
    tls.rule(
        "/manifest-stable.toml",
        Rule::Redirect(format!("{}/manifest-stable.toml", plain.url())),
    );
    let o = fx
        .cigi_with_server(&tls)
        .env("CIGI_ALLOW_SINGLE_SOURCE", "true")
        .args(["--yes", "update", "--check", "--refresh"])
        .output()
        .unwrap();
    assert_ne!(o.status.code(), Some(0), "{}", text(&o));
    assert!(
        text(&o).contains("redirect from https to http"),
        "{}",
        text(&o)
    );
    assert!(
        tls.paths().iter().any(|p| p.contains("manifest-stable")),
        "{:?}",
        tls.paths()
    );
    assert!(
        plain.requests().is_empty(),
        "the http target was never asked: {:?}",
        plain.paths()
    );
}

#[test]
fn https_mirror_with_an_unknown_certificate_is_refused() {
    let mut fx = fixture();
    two(&mut fx);
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    let tls = fx.serve_tls();
    // the server's certificate is in no trust store this run can see
    let o = fx
        .cigi_with_mirror(&tls.url())
        .env_remove("SSL_CERT_FILE")
        .env_remove("SSL_CERT_DIR")
        .env("CIGI_ALLOW_SINGLE_SOURCE", "true")
        .args(["--yes", "update", "--check", "--refresh"])
        .output()
        .unwrap();
    assert_ne!(o.status.code(), Some(0), "{}", text(&o));
    assert!(
        tls.requests().is_empty(),
        "nothing got past the handshake: {:?}",
        tls.paths()
    );
}
