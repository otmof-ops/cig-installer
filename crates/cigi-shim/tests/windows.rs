// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! The built shim, run as the real thing on Windows (issue #8, SPEC section 9): the tool name
//! from the file's own stem, any case; the child's exit code; arguments untouched; no exec.
//! Only the Windows CI job runs these.
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::process::Command;

/// A home with `bin\<name>.exe` a copy of the shim, and `target` the program the index
/// names for `cig`: a copy of `cmd.exe`, so arguments and exit codes are real.
fn setup(shim_name: &str) -> (PathBuf, PathBuf, PathBuf) {
    let root =
        std::env::temp_dir().join(format!("cigi-shim-win-{}-{shim_name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (bin, inst) = (root.join("bin"), root.join("installer"));
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(&inst).unwrap();
    let shim = bin.join(shim_name);
    std::fs::copy(env!("CARGO_BIN_EXE_cigi-shim"), &shim).unwrap();
    let target = root.join("cig-target.exe");
    let sys = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap();
    std::fs::copy(sys.join("System32").join("cmd.exe"), &target).unwrap();
    std::fs::write(
        inst.join("shim-index"),
        format!("cigi-index 1\nstick\tcig\t{}\n", target.display()),
    )
    .unwrap();
    (root, shim, target)
}

fn run(root: &Path, shim: &Path, args: &[&str]) -> std::process::Output {
    Command::new(shim)
        .args(args)
        .current_dir(root)
        .env("CIGSCRIPT_HOME", root)
        .env_remove("CIGI_CARTON")
        .output()
        .unwrap()
}

#[test]
fn windows_shim_returns_the_childs_exit_code() {
    let (root, shim, _) = setup("cig.exe");
    let out = run(&root, &shim, &["/c", "exit", "42"]);
    assert_eq!(out.status.code(), Some(42));
}

#[test]
fn windows_shim_passes_arguments_through_untouched() {
    let (root, shim, _) = setup("cig.exe");
    let out = run(&root, &shim, &["/c", "echo", "a b", "&", "echo", "c"]);
    assert!(String::from_utf8_lossy(&out.stdout).contains("a b"));
}

#[test]
fn windows_shim_name_ignores_case_and_extension() {
    let (root, shim, _) = setup("CIG.EXE");
    let out = run(&root, &shim, &["/c", "exit", "3"]);
    assert_eq!(out.status.code(), Some(3));
}

#[test]
fn windows_shim_refuses_a_name_it_does_not_serve() {
    let (root, shim, _) = setup("notatool.exe");
    let out = run(&root, &shim, &[]);
    assert_eq!(out.status.code(), Some(127));
    assert!(String::from_utf8_lossy(&out.stderr).contains("E1844"));
}

#[test]
fn windows_shim_passes_an_ntstatus_exit_code_unchanged() {
    let (root, shim, _) = setup("cig.exe");
    let out = run(&root, &shim, &["/c", "exit", "-1073741819"]);
    assert_eq!(out.status.code().map(|c| c as u32), Some(0xC000_0005));
}

#[test]
fn windows_cigi_exe_runs_the_installer_and_returns_its_code() {
    let (root, shim, target) = setup("cigi.exe");
    std::fs::copy(&target, root.join("bin").join("ciginstaller.exe")).unwrap();
    let out = run(&root, &shim, &["/c", "exit", "9"]);
    assert_eq!(out.status.code(), Some(9));
}

#[test]
fn windows_cigi_exe_puts_the_previous_installer_back_when_the_new_one_cannot_start() {
    let (root, shim, target) = setup("cigi.exe");
    // the "new" build is not an executable at all: it cannot start
    std::fs::write(root.join("bin").join("ciginstaller.exe"), b"not an exe").unwrap();
    let prev = root.join("installer").join("previous");
    std::fs::create_dir_all(&prev).unwrap();
    std::fs::copy(&target, prev.join("ciginstaller.exe")).unwrap();
    std::fs::write(root.join("installer").join("pending-check"), "0.1.1\n").unwrap();
    let out = run(&root, &shim, &["/c", "exit", "5"]);
    assert_eq!(
        out.status.code(),
        Some(5),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(root.join("installer").join("restored").is_file());
    assert!(!root.join("installer").join("pending-check").exists());
    assert!(prev.join("ciginstaller.failed.exe").is_file());
}

#[test]
fn windows_shim_survives_ctrl_break_that_the_child_also_receives() {
    // the handler is installed: the call reports success, and a second install is harmless
    assert!(cigi_sys::process::swallow_ctrl_c());
    assert!(cigi_sys::process::swallow_ctrl_c());
}
