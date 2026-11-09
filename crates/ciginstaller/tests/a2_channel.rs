// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0
//! `channel` and `install --channel` (SPEC 10; issue #3): the new channel's manifest is fetched
//! and checked before the setting changes, and nothing is installed by a switch.

mod common;

use cigi_testkit::PublishOpts;
use common::{cigi, fixture, ok, out, text};

#[test]
fn a2_channel_shows_switches_and_refuses() {
    let mut fx = fixture();
    fx.carton("0.1.0", &[("cig", "1.20.0")]);
    fx.publish("stable");
    fx.carton("0.2.0", &[("cig", "1.21.0")]);
    fx.publish("beta");

    let o = cigi(&fx, &["--plain", "channel"]);
    ok(&o);
    assert!(
        out(&o).contains("follows the stable channel"),
        "{}",
        text(&o)
    );

    // a name outside the three, and a channel with no manifest: refused, setting unchanged
    for (name, want) in [
        ("bogus", "no channel `bogus`"),
        ("nightly", "no manifest yet"),
    ] {
        let o = cigi(&fx, &["--plain", "channel", name]);
        assert_eq!(o.status.code(), Some(2), "{name}: {}", text(&o));
        assert!(text(&o).contains("E1812"), "{}", text(&o));
        assert!(text(&o).contains(want), "{}", text(&o));
    }
    let config = fx.cigscript().join("installer/config");
    assert!(
        !std::fs::read_to_string(&config)
            .unwrap_or_default()
            .contains("channel"),
        "a refused switch wrote the setting"
    );

    // beta has a manifest: switched, newest named, nothing installed
    let o = cigi(&fx, &["--plain", "channel", "beta"]);
    ok(&o);
    assert!(
        out(&o).contains("now follows the beta channel"),
        "{}",
        text(&o)
    );
    assert!(out(&o).contains("0.2.0"), "{}", text(&o));
    assert!(std::fs::read_to_string(&config)
        .unwrap()
        .contains("channel = beta"));
    let installed = std::fs::read_dir(fx.cigscript().join("toolchains"))
        .map(|d| d.count())
        .unwrap_or(0);
    assert_eq!(installed, 0, "a channel switch installed something");
    let o = cigi(&fx, &["--plain", "channel", "beta"]);
    ok(&o);
    assert!(out(&o).contains("already follows"), "{}", text(&o));
    let o = cigi(&fx, &["--json", "channel"]);
    ok(&o);
    assert!(out(&o).contains("\"channel\":\"beta\""), "{}", text(&o));
}

#[test]
fn a2_install_channel_is_for_one_run() {
    let mut fx = fixture();
    fx.carton("0.1.0", &[("cig", "1.20.0")]);
    fx.publish("stable");
    fx.carton("0.2.0", &[("cig", "1.21.0")]);
    fx.publish("beta");

    ok(&cigi(&fx, &["install", "carton", "--channel", "beta"]));
    assert!(fx.cigscript().join("toolchains/cig/1.21.0").is_dir());
    let o = cigi(&fx, &["--plain", "channel"]);
    assert!(out(&o).contains("stable"), "{}", text(&o));

    let o = cigi(&fx, &["install", "carton", "--channel", "weekly"]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(text(&o).contains("E1812"), "{}", text(&o));
}

/// A channel's own cache keeps its sequence floor across switches: an older document served for
/// a channel this machine has seen at a higher sequence is refused, even after it followed
/// another channel in between (the security re-review of PR #37).
#[test]
fn a2_channel_keeps_each_channels_floor() {
    let mut fx = fixture();
    fx.carton("0.1.0", &[("cig", "1.20.0")]);
    fx.publish("stable");
    fx.carton("0.2.0", &[("cig", "1.21.0")]);
    fx.publish("beta");
    fx.publish("beta"); // beta is at sequence 2
    ok(&cigi(&fx, &["channel", "beta"]));
    ok(&cigi(&fx, &["channel", "stable"]));
    fx.publish_with(
        "beta",
        &PublishOpts {
            sequence: Some(1),
            ..PublishOpts::default()
        },
    );
    let o = cigi(&fx, &["--plain", "channel", "beta"]);
    assert_ne!(o.status.code(), Some(0), "{}", text(&o));
    assert!(text(&o).contains("E1824"), "{}", text(&o));
    let o = cigi(&fx, &["--plain", "channel"]);
    assert!(out(&o).contains("stable"), "{}", text(&o));
}
