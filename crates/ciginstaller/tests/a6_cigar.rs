// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! CigarScript in the carton (#1, #3): a carton that names a cigar build installs it, shims it,
//! resolves a project's `.cigar-version` pin, and `uninstall cigar` takes the shim, the toolchain
//! and cigar's run cache. `doctor --tools` reads cigar's own `__doctor --json`.
//!
//! The products are the fixture's fakes: shell scripts, so these tests are Unix only.
#![cfg(unix)]

mod common;

use std::path::Path;

use cigi_testkit::Fixture;
use common::{cigi, fixture, ok, text};
use serde_json::Value;

const DOCTOR: &str = r#"{"tool":"cigar","version":"0.2.0","findings":[
  {"check":"lowering","severity":"ok","found":"the sample lowers","because":"no internal error","fix":null},
  {"check":"embedded-cig","severity":"note","found":"cigar embeds cig 1.18.0, cig 1.20.0 is installed","because":"the run cache is built from the embedded one","fix":"ciginstaller check"}]}"#;

/// Two cartons: 0.1.0 has cigar 0.1.0, 0.2.0 has cigar 0.2.0 with a self-check. Both installed,
/// the machine on 0.1.0.
fn setup() -> Fixture {
    let mut fx = fixture();
    fx.product("cigar", "0.2.0").doctor_json(DOCTOR);
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigar", "0.1.0")]);
    fx.carton("0.2.0", &[("cig", "1.20.0"), ("cigar", "0.2.0")]);
    fx.publish("stable");
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.2.0"]));
    fx
}

fn project(fx: &Fixture, name: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
    let p = fx.tmp.path().join(name);
    std::fs::create_dir_all(&p).unwrap();
    for (f, c) in files {
        std::fs::write(p.join(f), c).unwrap();
    }
    p
}

fn which(fx: &Fixture, cwd: &Path, args: &[&str]) -> std::process::Output {
    let mut c = fx.cigi();
    c.env("CIGI_ALLOW_SINGLE_SOURCE", "true")
        .current_dir(cwd)
        .arg("which")
        .args(args);
    cigi_testkit::output_retrying(&mut c).expect("run which")
}

fn json(o: &std::process::Output) -> Value {
    let line = String::from_utf8_lossy(&o.stdout)
        .lines()
        .last()
        .unwrap_or("")
        .to_string();
    let v: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {}", text(o)));
    v["data"].clone()
}

#[test]
fn a_carton_with_cigar_installs_and_shims_it() {
    let fx = setup();
    let root = fx.cigscript();
    for v in ["0.1.0", "0.2.0"] {
        assert!(
            root.join("toolchains/cigar")
                .join(v)
                .join("bin/cigar")
                .is_file(),
            "toolchains/cigar/{v}"
        );
    }
    assert!(root.join("bin/cigar").exists(), "the cigar shim");
    // the machine's carton is 0.1.0, so the shim runs cigar 0.1.0
    let o = fx.run_shim("cigar", &fx.project, &["check"]);
    ok(&o);
    assert!(
        text(&o).starts_with("cigar 0.1.0 args=check"),
        "{}",
        text(&o)
    );
    let o = which(&fx, &fx.project, &["cigar"]);
    ok(&o);
    assert!(
        text(&o).contains("CigarScript 0.1.0, from carton 0.1.0"),
        "{}",
        text(&o)
    );
    let idx = std::fs::read_to_string(root.join("installer/shim-index")).unwrap();
    assert!(idx.contains("\tcigar\t"), "{idx}");
    assert!(idx.contains(".cigar-version"), "{idx}");
    // the archive also carries the language server, and its shim runs the carton's: an editor
    // that starts `cigar-lsp` in a project gets the one the project's cigar came with
    assert!(
        root.join("toolchains/cigar/0.1.0/bin/cigar-lsp").is_file(),
        "toolchains/cigar/0.1.0/bin/cigar-lsp"
    );
    assert!(root.join("bin/cigar-lsp").exists(), "the cigar-lsp shim");
    // prove the fast path: the slow path would need bin/ciginstaller to reach the resolver,
    // so moving it aside leaves only the fast path's own index to serve the shim
    let installer = root.join("bin/ciginstaller");
    let aside = root.join("bin/ciginstaller.aside");
    std::fs::rename(&installer, &aside).unwrap();
    let o = fx.run_shim("cigar-lsp", &fx.project, &[]);
    std::fs::rename(&aside, &installer).unwrap();
    ok(&o);
    assert!(
        text(&o).starts_with("cigar-lsp 0.1.0 args="),
        "{}",
        text(&o)
    );
    assert!(idx.contains("\tcigar-lsp\t"), "{idx}");
}

#[test]
fn a_cigar_version_pin_picks_the_carton_that_satisfies_it() {
    let fx = setup();
    let proj = project(&fx, "pinned", &[(".cigar-version", ">=0.2\n")]);
    let o = which(&fx, &proj, &["cigar"]);
    ok(&o);
    assert!(
        text(&o)
            .contains("CigarScript 0.2.0, from carton 0.2.0 (satisfies .cigar-version >=0.2.0)"),
        "{}",
        text(&o)
    );
    let o = fx.run_shim("cigar", &proj, &["run", "x.cigar"]);
    ok(&o);
    assert!(
        text(&o).starts_with("cigar 0.2.0 args=run x.cigar"),
        "{}",
        text(&o)
    );
    // a pin the installed builds do not meet is refused with the fix, never run
    let none = project(&fx, "none", &[(".cigar-version", ">=3.0\n")]);
    let o = fx.run_shim("cigar", &none, &[]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(text(&o).contains("E1801"), "{}", text(&o));
}

#[test]
fn doctor_tools_reads_cigars_self_check() {
    let fx = setup();
    // the machine is on 0.1.0, which has no self-check: a note, never a call to `cigar doctor`
    let mut c = fx.cigi();
    c.env("CIGI_ALLOW_SINGLE_SOURCE", "true")
        .args(["--yes", "--json", "doctor", "--tools"]);
    let o = cigi_testkit::output_retrying(&mut c).unwrap();
    let d = json(&o);
    let cigar = d["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["tool"].as_str().unwrap().starts_with("cigar"))
        .unwrap_or_else(|| panic!("no cigar in {d:#}"))
        .clone();
    assert_eq!(cigar["tool"], "cigar 0.1.0", "{cigar:#}");
    assert_eq!(cigar["self_check"], false);
    // move the machine to 0.2.0: its findings show, the embedded-cig note with its fix
    ok(&cigi(&fx, &["use", "0.2.0"]));
    let mut c = fx.cigi();
    c.env("CIGI_ALLOW_SINGLE_SOURCE", "true")
        .args(["--yes", "--json", "doctor", "--tools"]);
    let d = json(&cigi_testkit::output_retrying(&mut c).unwrap());
    let cigar = d["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["tool"].as_str().unwrap().starts_with("cigar"))
        .unwrap()
        .clone();
    assert_eq!(cigar["self_check"], true, "{cigar:#}");
    let f = cigar["findings"].as_array().unwrap();
    assert_eq!(f.len(), 2);
    assert_eq!(f[1]["severity"], "note");
    assert_eq!(f[1]["fix"], "ciginstaller check");
}

#[test]
fn uninstall_cigar_clears_shim_toolchain_and_run_cache() {
    let fx = setup();
    let root = fx.cigscript();
    let cache = fx.user_home.join(".cache/cigar/run");
    std::fs::create_dir_all(&cache).unwrap();
    std::fs::write(cache.join("prog-1"), "built\n").unwrap();
    std::fs::create_dir_all(fx.user_home.join(".cache/other")).unwrap();
    std::fs::write(fx.user_home.join(".cache/other/keep"), "x\n").unwrap();
    // the language server's shim is in bin/ before uninstall and it follows cigar out
    assert!(root.join("bin/cigar-lsp").exists(), "the cigar-lsp shim");
    let o = cigi(&fx, &["--json", "uninstall", "cigar"]);
    ok(&o);
    assert!(!root.join("toolchains/cigar").exists());
    assert!(!root.join("bin/cigar").exists(), "the shim goes with it");
    assert!(
        !root.join("bin/cigar-lsp").exists(),
        "the cigar-lsp shim goes with it"
    );
    assert!(!cache.exists(), "the run cache");
    assert!(fx.user_home.join(".cache/other/keep").exists());
    assert!(root.join("bin/cig").exists(), "cig stays");
    let removed = json(&o)["removed"].to_string();
    assert!(removed.contains("cigar's run cache"), "{removed}");
    let idx = std::fs::read_to_string(root.join("installer/shim-index")).unwrap();
    assert!(!idx.contains("\tcigar\t"), "{idx}");
    assert!(!idx.contains("\tcigar-lsp\t"), "{idx}");
}

#[test]
fn a_cigar_version_pin_resolves_cigar_lsp_through_the_slow_path() {
    // an editor that starts `cigar-lsp` in a project gets the server of the cigar the project
    // resolves to: a `.cigar-version` pin past the machine's carton sends the shim to the slow
    // path (as it does for `cigar`), and that path runs the cigar-lsp of the carton the pin
    // picks (cigar 0.2.0's language server).
    let fx = setup();
    let proj = project(&fx, "pinned-lsp", &[(".cigar-version", ">=0.2\n")]);
    let o = fx.run_shim("cigar-lsp", &proj, &[]);
    ok(&o);
    assert!(
        text(&o).starts_with("cigar-lsp 0.2.0 args="),
        "{}",
        text(&o)
    );
    let o = which(&fx, &proj, &["cigar-lsp"]);
    ok(&o);
    assert!(text(&o).contains("cigar-lsp 0.2.0"), "{}", text(&o));
}

#[test]
fn repair_recreates_a_deleted_cigar_lsp_shim() {
    let fx = setup();
    let root = fx.cigscript();
    let lsp = root.join("bin/cigar-lsp");
    assert!(lsp.exists(), "the cigar-lsp shim is there before repair");
    std::fs::remove_file(&lsp).unwrap();
    assert!(!lsp.exists());
    ok(&cigi(&fx, &["repair"]));
    assert!(lsp.exists(), "repair puts it back");
    // and it still runs the carton's cigar-lsp, through the fast path
    let o = fx.run_shim("cigar-lsp", &fx.project, &[]);
    ok(&o);
    assert!(
        text(&o).starts_with("cigar-lsp 0.1.0 args="),
        "{}",
        text(&o)
    );
}

#[test]
fn an_index_from_before_the_change_still_runs_cigar_fast_and_cigar_lsp_slow() {
    // A machine that has installed cigar when the installer's index knew only cigar rows
    // (header is still `cigi-index 2`): the fast path finds `cigar`, and `cigar-lsp` falls
    // through to __resolve which runs the shim's target in the same toolchain.
    let fx = setup();
    let root = fx.cigscript();
    // rewrite the index with no cigar-lsp rows, preserving the identity on the cigar rows
    let before = std::fs::read_to_string(root.join("installer/shim-index")).unwrap();
    assert!(before.starts_with("cigi-index 2\n"));
    let trimmed: String = before
        .lines()
        .filter(|l| !l.contains("\tcigar-lsp\t"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    assert!(trimmed.contains("\tcigar\t"), "{trimmed}");
    assert!(!trimmed.contains("\tcigar-lsp\t"), "{trimmed}");
    std::fs::write(root.join("installer/shim-index"), &trimmed).unwrap();
    // cigar still runs fast: the ciginstaller binary is beside the shim; moving it aside
    // proves the fast path answers without reaching the resolver
    let installer = root.join("bin/ciginstaller");
    let aside = root.join("bin/ciginstaller.aside");
    std::fs::rename(&installer, &aside).unwrap();
    let o = fx.run_shim("cigar", &fx.project, &[]);
    std::fs::rename(&aside, &installer).unwrap();
    ok(&o);
    assert!(text(&o).starts_with("cigar 0.1.0 args="), "{}", text(&o));
    // cigar-lsp takes the slow path: __resolve runs the shim's target in the same toolchain
    let o = fx.run_shim("cigar-lsp", &fx.project, &[]);
    ok(&o);
    assert!(
        text(&o).starts_with("cigar-lsp 0.1.0 args="),
        "{}",
        text(&o)
    );
}

#[test]
fn a_cigar_that_lacks_cigar_lsp_refuses_the_shim_with_e1840() {
    // An older cigar release whose manifest lists only `cigar` and whose archive carries only
    // `bin/cigar`: no bin/cigar-lsp is placed, and a `which cigar-lsp` run says E1840 (exit 127)
    // naming cigar as the tool the shim is beside.
    let mut fx = fixture();
    fx.cigar_without_lsp = true;
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigar", "0.1.0")]);
    fx.publish("stable");
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    let root = fx.cigscript();
    assert!(root.join("bin/cigar").exists(), "the cigar shim");
    assert!(
        !root.join("bin/cigar-lsp").exists(),
        "no cigar-lsp shim when the cigar release does not ship it"
    );
    assert!(
        !root.join("toolchains/cigar/0.1.0/bin/cigar-lsp").exists(),
        "no cigar-lsp binary in the toolchain either"
    );
    let idx = std::fs::read_to_string(root.join("installer/shim-index")).unwrap();
    assert!(idx.contains("\tcigar\t"), "{idx}");
    assert!(!idx.contains("\tcigar-lsp\t"), "{idx}");
    // which cigar-lsp refuses with E1840 and exit 127 (EXIT_SHIM)
    let mut c = fx.cigi();
    c.env("CIGI_ALLOW_SINGLE_SOURCE", "true")
        .args(["which", "cigar-lsp"]);
    let o = cigi_testkit::output_retrying(&mut c).unwrap();
    assert_eq!(o.status.code(), Some(127), "{}", text(&o));
    assert!(text(&o).contains("E1840"), "{}", text(&o));
    // product_names rewrites `cigar` into CigarScript before a version or at the end of the
    // phrase (`or use cigar`); the shim name `cigar-lsp` stays as the hyphen is after_glue
    assert!(text(&o).contains("0.1.0 has no cigar-lsp"), "{}", text(&o));
}
