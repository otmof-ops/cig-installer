// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `install pack dev` against the real budget (#20: under 30 s on broadband), from a local mirror
//! on a release build: the installer's own part, which the network only adds to. The debug-build
//! check is `a2_perf_install` in `ciginstaller/tests/a2.rs`, which the release build cannot run.
#![cfg(unix)]

#[path = "../../ciginstaller/tests/common/mod.rs"]
mod common;

use std::time::Instant;

use common::*;

#[test]
fn budget_install_pack_dev_from_a_local_mirror() {
    let limit = if cfg!(debug_assertions) { 5.0 } else { 3.0 };
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
        "budget_install_pack_dev: best {best:.2} s from a local mirror (budget 30 s on broadband)"
    );
    assert!(
        best < limit,
        "the fastest of three installs took {best:.2} s"
    );
}
