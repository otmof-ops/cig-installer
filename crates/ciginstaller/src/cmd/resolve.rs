// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0
//! `__resolve`: the shim's slow path (SPEC 9). Resolve, then run the tool with the carton in its
//! environment and exit with its code. Runs the tool as a child (no `cfg(unix)` outside the
//! platform layer, D9); the shim's fast path execs.

use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

use cigi_ui::{InstallerError, EXIT_SHIM};

use crate::cx::Cx;
use crate::resolve::{resolve, Mode};

/// Returns the tool's exit code, or an error when it could not be found or started.
pub fn run(cx: &mut Cx, tool: &str, args: &[OsString]) -> Result<i32, InstallerError> {
    let res = resolve(cx, tool, Mode::Run)?;
    super::which::touch_last_used(cx, &res);
    let mut cmd = Command::new(&res.path);
    cmd.args(args).env("CIGI_MANAGED", "1");
    if let Some(c) = &res.carton {
        cmd.env("CIGI_CARTON", c.to_string());
    }
    match cmd.status() {
        Ok(s) => Ok(s.code().unwrap_or(1)),
        Err(e) => Err(InstallerError::new(
            "E1831",
            format!("cannot start {}: {e}", Path::new(&res.path).display()),
        )
        .with_fix("ciginstaller repair")
        .with_exit(EXIT_SHIM)),
    }
}
