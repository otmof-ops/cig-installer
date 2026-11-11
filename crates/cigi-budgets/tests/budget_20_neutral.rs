// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! The budgets of #20 that need no shell-script product, so they run on every platform, Windows
//! included (`budget_20.rs` is unix-only because its fixture products are shell scripts): the shim's
//! dispatch, the atomic switch, the two binary sizes, and the disk five cigo versions cost, read
//! from receipts written straight into a scratch home. `ciginstaller bench` does the measuring;
//! these tests run it in a scratch `HOME` and `CIGSCRIPT_HOME` and read its `--json` report.
//!
//! On a debug build (a plain `cargo test`) the timings are only checked for being measured and sane,
//! and the sizes are not judged, because the budgets are a release build's. On a release build
//! (`cargo test --release`, which the `bench` workflow runs on Linux, macOS and Windows) each row
//! must be `ok` at its real limit. A row that cannot be measured says "not measured" and is never
//! counted as passing (see `a_row_that_is_not_measured_never_reads_as_ok`).

#[path = "../../ciginstaller/tests/common/mod.rs"]
mod common;

use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use cigi_testkit::binary_path;
use serde_json::{json, Value};

/// A child that does not finish in this long is a hang, not a slow row.
const LIMIT: Duration = Duration::from_secs(180);

struct Scratch {
    _dir: tempfile::TempDir,
    user: std::path::PathBuf,
    home: std::path::PathBuf,
}

fn scratch() -> Scratch {
    common::ensure_shim();
    let dir = tempfile::tempdir().expect("a scratch directory");
    let user = dir.path().join("user");
    let home = dir.path().join("cigscript");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    Scratch {
        _dir: dir,
        user,
        home,
    }
}

fn run_with_timeout(mut c: Command) -> Output {
    c.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = c.spawn().expect("run ciginstaller");
    let (tx, rx) = mpsc::channel();
    let id = child.id();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    match rx.recv_timeout(LIMIT) {
        Ok(o) => o.expect("wait for ciginstaller"),
        Err(_) => panic!("ciginstaller (pid {id}) did not finish in {LIMIT:?}"),
    }
}

fn bench(s: &Scratch, args: &[&str]) -> Value {
    let mut c = Command::new(binary_path("ciginstaller"));
    c.env_clear()
        .env("HOME", &s.user)
        .env("USERPROFILE", &s.user)
        .env("APPDATA", s.user.join("AppData"))
        .env("LOCALAPPDATA", s.user.join("LocalAppData"))
        .env("CIGSCRIPT_HOME", &s.home)
        .env("CIGI_NO_REGISTRY_PATH", "1")
        .env("NO_COLOR", "1")
        .current_dir(&s.user)
        .args(["--json", "bench", "--runs", "30"])
        .args(args);
    if let Some(root) = std::env::var_os("SystemRoot") {
        c.env("SystemRoot", root);
    }
    let o = run_with_timeout(c);
    let text = String::from_utf8_lossy(&o.stdout).into_owned();
    let v: Value = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("{e}: {text}{}", String::from_utf8_lossy(&o.stderr)));
    assert!(o.status.success(), "bench failed: {text}");
    v["data"].clone()
}

fn row<'a>(d: &'a Value, id: &str) -> &'a Value {
    d["budgets"]
        .as_array()
        .and_then(|b| b.iter().find(|r| r["id"] == id))
        .unwrap_or_else(|| panic!("no budget row {id} in {d}"))
}

fn release() -> bool {
    !cfg!(debug_assertions)
}

/// Five receipts for cigo: `shared` bytes common to all, plus one file of its own each.
fn write_five_cigo(s: &Scratch, shared: bool) {
    let digest = |tag: &str| {
        format!(
            "{:0>64}",
            tag.chars()
                .map(|c| format!("{:x}", c as u32))
                .collect::<String>()
        )
    };
    for (i, v) in ["1.17.0", "1.18.0", "1.19.0", "1.20.0", "1.21.0"]
        .iter()
        .enumerate()
    {
        let dir = s.home.join("toolchains").join("cigo").join(v);
        std::fs::create_dir_all(&dir).unwrap();
        let common_file = if shared {
            digest("shared")
        } else {
            digest(&format!("own-{i}"))
        };
        let receipt = json!({
            "format": 1,
            "tool": "cigo",
            "version": v,
            "installed_at": "2026-01-01T00:00:00Z",
            "from_carton": null,
            "txn": "bench",
            "source": {"repo": "otmof-ops/cig-orchestrator", "tag": format!("v{v}"), "asset": "cigo", "origin": "mirror"},
            "artifact": {"sha256": digest("artifact"), "size": 65_600},
            "trust": {"level": "checksummed", "independent": false, "attested_by": null, "manifest_sequence": 1},
            "files": [
                {"path": "data/shared.bin", "sha256": common_file, "exec": false, "bytes": 65_536},
                {"path": "bin/cigo", "sha256": digest(&format!("bin-{i}")), "exec": true, "bytes": 64}
            ],
            "shims": [],
            "humidor_mirror": null
        });
        std::fs::write(
            dir.join(".cigi-receipt.json"),
            serde_json::to_vec_pretty(&receipt).unwrap(),
        )
        .unwrap();
    }
}

/// The shim's dispatch, the atomic switch and both binary sizes: no shell-script product needed.
#[test]
fn budget_neutral_shim_switch_and_sizes() {
    let s = scratch();
    let d = bench(&s, &[]);
    let platform = std::env::consts::OS;
    for id in ["shim_dispatch", "switch", "installer_bytes", "shim_bytes"] {
        let r = row(&d, id);
        eprintln!(
            "budget_neutral[{platform}]: {id} median {} {} (limit {}, {}) status {}",
            r["median"],
            r["unit"].as_str().unwrap_or(""),
            r["limit"],
            d["build"].as_str().unwrap_or(""),
            r["status"].as_str().unwrap_or("")
        );
        assert!(r["median"].is_number(), "{id} was not measured: {r}");
        if release() {
            assert_eq!(r["status"], "ok", "{id} is over its budget: {r}");
        } else {
            // a debug build is slower and bigger: measured, but the budget is the release build's
            assert_ne!(r["status"], "not measured", "{r}");
        }
    }
    assert_eq!(d["build"], if release() { "release" } else { "debug" });
}

/// Five cigo versions that share their bulk cost under 1.5 times one: judged `ok` from the receipts.
#[test]
fn budget_neutral_disk_for_five_cigo_versions() {
    let s = scratch();
    write_five_cigo(&s, true);
    let d = bench(&s, &[]);
    let r = row(&d, "disk_five_cigo");
    let ratio = r["median"]
        .as_f64()
        .unwrap_or_else(|| panic!("not measured: {r}"));
    eprintln!(
        "budget_neutral[{}]: disk for five cigo versions {ratio:.3}x one (budget 1.5x)",
        std::env::consts::OS
    );
    assert!((1.0..1.5).contains(&ratio), "{r}");
    assert_eq!(r["status"], "ok", "{r}");
}

/// The row can fail: five versions that share nothing cost about five times one.
#[test]
fn budget_neutral_disk_row_goes_over_when_nothing_is_shared() {
    let s = scratch();
    write_five_cigo(&s, false);
    let d = bench(&s, &[]);
    let r = row(&d, "disk_five_cigo");
    assert_eq!(r["status"], "over", "{r}");
    assert!(r["median"].as_f64().unwrap_or(0.0) > 1.5, "{r}");
}

/// What this platform cannot measure yet is printed as not measured, never as passing: with no
/// cached metadata, no held versions and no mirror, `check`, the disk row and both installs say so.
#[test]
fn a_row_that_is_not_measured_never_reads_as_ok() {
    let s = scratch();
    let d = bench(&s, &[]);
    for id in [
        "check",
        "disk_five_cigo",
        "install_pack_dev",
        "install_carton",
    ] {
        let r = row(&d, id);
        assert_eq!(r["status"], "not measured", "{id}: {r}");
        assert!(
            r["median"].is_null(),
            "{id} has a number it did not measure: {r}"
        );
        assert!(
            !r["note"].as_str().unwrap_or("").is_empty(),
            "{id} does not say why: {r}"
        );
    }
    let _: &Path = &s.home;
}
