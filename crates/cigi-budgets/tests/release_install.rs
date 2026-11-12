// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `install pack dev` against the real budget (#20: under 30 s on broadband), from a local mirror
//! on a release build: the installer's own part, which the network only adds to. This is the one
//! place the install's time is judged; the functional suite (`a2_install_pack_dev_on_a_fresh_home`
//! in `ciginstaller/tests/a2.rs`) checks only what the install leaves. On a debug build (a plain
//! `cargo test`) the time is measured and printed, never judged, because the budget is a release
//! build's and a slow machine is not a regression.
#![cfg(unix)]

#[path = "../../ciginstaller/tests/common/mod.rs"]
mod common;

use std::time::Instant;

use common::*;

#[test]
fn budget_install_pack_dev_from_a_local_mirror() {
    // the installer's own part of the 30 s, from a local mirror on a release build
    let limit = 3.0;
    let judged = !cfg!(debug_assertions);
    let mut best = f64::MAX;
    for _ in 0..3 {
        let mut fx = fixture();
        fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.17.0")]);
        fx.publish("stable");
        let t = Instant::now();
        ok(&cigi(&fx, &["install", "pack", "dev"]));
        best = best.min(t.elapsed().as_secs_f64());
        if best < limit / 2.0 {
            break;
        }
    }
    eprintln!(
        "budget_install_pack_dev: best {best:.2} s from a local mirror (limit {limit} s on a release build; budget 30 s on broadband){}",
        if judged { "" } else { "; a debug build, not judged" }
    );
    assert!(best.is_finite() && best > 0.0, "nothing was measured");
    if judged {
        assert!(
            best < limit,
            "the fastest of three installs took {best:.2} s"
        );
    }
}
