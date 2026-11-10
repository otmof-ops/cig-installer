// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! Shared helpers for the acceptance tests: a fixture whose installer can find its shim.
#![allow(dead_code)]

use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::Once;

use cigi_testkit::Fixture;

static BUILD_SHIM: Once = Once::new();

/// `cigi-shim` is a binary of another package, so cargo does not build it for these tests.
/// Build it once beside the installer (the installer looks for it next to itself).
pub fn ensure_shim() {
    BUILD_SHIM.call_once(|| {
        let exe = std::env::current_exe().unwrap_or_default();
        let profile_dir = exe
            .parent()
            .and_then(|d| d.parent())
            .map(PathBuf::from)
            .unwrap_or_default();
        let shim = profile_dir.join(format!("cigi-shim{}", std::env::consts::EXE_SUFFIX));
        if shim.is_file() {
            return;
        }
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut cmd = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
        cmd.current_dir(root)
            .args(["build", "-p", "cigi-shim", "--offline"]);
        if profile_dir.file_name().is_some_and(|n| n == "release") {
            cmd.arg("--release");
        }
        if let Some(target) = profile_dir.parent() {
            cmd.arg("--target-dir").arg(target);
        }
        let st = cmd.status().expect("run cargo to build cigi-shim");
        assert!(st.success(), "building cigi-shim failed");
    });
}

pub fn fixture() -> Fixture {
    ensure_shim();
    Fixture::new()
}

pub fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

/// Run the installer with `--yes` and return the output; panics when it cannot start.
pub fn cigi(fx: &Fixture, args: &[&str]) -> Output {
    fx.cigi()
        // The fixture's releases folder serves the manifest, the checksums and the files
        // itself: one source, accepted only with the opt-in (`a2_single_source_refused`
        // proves the default).
        .env("CIGI_ALLOW_SINGLE_SOURCE", "true")
        .arg("--yes")
        .args(args)
        .output()
        .expect("run ciginstaller")
}

pub fn ok(o: &Output) {
    assert_eq!(o.status.code(), Some(0), "{}", text(o));
}

pub fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}
