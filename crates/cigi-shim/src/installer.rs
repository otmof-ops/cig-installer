// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! `bin\cigi.exe` on Windows (SPEC section 12.4, issue #8). Windows has no second name for a
//! running file the way a Unix hard link is one, so `cigi.exe` is this shim and not a copy of the
//! installer. It runs `bin\ciginstaller.exe` and returns its exit code. That is the whole of what
//! it does, except in one window: after a self-update has committed and before the new build has
//! run one successful command (`installer\pending-check` is there). If the new build then cannot
//! start, the shim puts `installer\previous\ciginstaller.exe` back, says so, leaves
//! `installer\restored` for the installer to reconcile its state with, and runs the build it put
//! back. That is the one thing the shim ever writes.
//!
//! The functions take what they touch as arguments and compile everywhere, so Linux tests cover them.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use cigi_shim::{FAILED_INSTALLER, PENDING_CHECK, PREVIOUS_INSTALLER, RESTORED};

pub const INSTALLER_EXE: &str = "ciginstaller.exe";

fn under(home: &Path, parts: &[&str]) -> PathBuf {
    parts.iter().fold(home.to_path_buf(), |p, s| p.join(s))
}

/// The name a shim runs under: its file's stem, without `.exe`. Windows file names are not
/// case-sensitive, so `CIG.EXE` is `cig` there (`fold`); elsewhere the name is exact.
pub fn tool_name(path: &Path, fold: bool) -> String {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    if fold {
        stem.to_ascii_lowercase()
    } else {
        stem.to_string()
    }
}

/// Exit codes that say the program never got going: a DLL it needs is missing, its image is
/// bad, its entry point is absent or its initialization failed (`STATUS_DLL_NOT_FOUND`,
/// `STATUS_INVALID_IMAGE_FORMAT`, `STATUS_ENTRYPOINT_NOT_FOUND`, `STATUS_DLL_INIT_FAILED`,
/// `STATUS_INVALID_IMAGE_NOT_MZ`, `STATUS_INVALID_IMAGE_PROTECT`).
pub fn never_started(code: Option<i32>) -> bool {
    matches!(
        code.map(|c| c as u32),
        Some(0xC000_0135 | 0xC000_007B | 0xC000_0139 | 0xC000_0142 | 0xC000_012F | 0xC000_0020)
    )
}

/// Any exit code in the NTSTATUS error range: the program died instead of finishing.
pub fn crashed(code: Option<i32>) -> bool {
    code.is_some_and(|c| (c as u32) & 0xC000_0000 == 0xC000_0000)
}

/// The version named in `installer\pending-check`, when a restore is possible: the marker is
/// there and so is the build to put back.
pub fn restorable(home: &Path) -> Option<String> {
    let text = fs::read_to_string(under(home, &PENDING_CHECK)).ok()?;
    under(home, &PREVIOUS_INSTALLER)
        .is_file()
        .then(|| text.trim().to_string())
}

fn retry<T>(mut f: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    let mut tries = 0;
    loop {
        match f() {
            // a scanner can hold a file for a moment
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied && tries < 10 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(100));
            }
            r => return r,
        }
    }
}

/// Put the previous installer back: the build that failed is kept beside it, `restored` records
/// the version that failed, and the marker that licensed all this is removed.
pub fn restore(home: &Path, failed_version: &str) -> io::Result<()> {
    let bin = home.join("bin").join(INSTALLER_EXE);
    let previous = under(home, &PREVIOUS_INSTALLER);
    let failed = under(home, &FAILED_INSTALLER);
    let _ = fs::remove_file(&failed);
    match retry(|| fs::rename(&bin, &failed)) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    if let Err(e) = retry(|| fs::rename(&previous, &bin)) {
        let _ = fs::rename(&failed, &bin);
        return Err(e);
    }
    fs::write(under(home, &RESTORED), format!("{failed_version}\n"))?;
    let _ = fs::remove_file(under(home, &PENDING_CHECK));
    Ok(())
}

/// Run the installer through `spawn` (which starts it and waits, giving its exit code) and
/// return the code to exit with. `Err` is "it could not be started and nothing could be put back".
pub fn run_with(
    home: &Path,
    spawn: &mut dyn FnMut() -> io::Result<Option<i32>>,
    grace: Duration,
) -> io::Result<i32> {
    let mut first = spawn();
    // `bin\ciginstaller.exe` is absent for a moment while an update swaps it in
    if matches!(&first, Err(e) if e.kind() == io::ErrorKind::NotFound) && restorable(home).is_some()
    {
        std::thread::sleep(grace);
        first = spawn();
    }
    let bad = match &first {
        Ok(code) => crashed(*code),
        Err(_) => true,
    };
    let version = if bad { restorable(home) } else { None };
    let (Some(version), true) = (version, bad) else {
        return first.map(|c| c.unwrap_or(1));
    };
    let reran = match &first {
        Ok(code) => never_started(*code),
        Err(_) => true,
    };
    if restore(home, &version).is_err() {
        return first.map(|c| c.unwrap_or(1));
    }
    eprintln!(
        "note: ciginstaller {version} could not start; the previous build was put back \
         (the one that failed is installer\\previous\\ciginstaller.failed.exe)"
    );
    if reran {
        spawn().map(|c| c.unwrap_or(1))
    } else {
        eprintln!("note: it stopped part-way; run the command again");
        first.map(|c| c.unwrap_or(1))
    }
}

/// Run `bin\ciginstaller.exe` with `args` and wait for it.
pub fn run(home: &Path, args: &[OsString]) -> io::Result<i32> {
    let exe = home.join("bin").join(INSTALLER_EXE);
    run_with(
        home,
        &mut || Command::new(&exe).args(args).status().map(|s| s.code()),
        Duration::from_millis(500),
    )
}
