// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0
//! The project a command runs in: the pin file the shim would see (SPEC 6.5), found the same way.

use std::path::{Path, PathBuf};

use cig_compat::Pin;
use cigi_shim::{
    find, read_capped, trusted_entry, trusted_for, Found, Reading, MAX_DEPTH, MAX_FILE_INSTALLER,
};
use cigi_ui::{InstallerError, EXIT_USAGE};

/// The per-tool pin files the shim sends to the slow path, and the tool each one pins.
pub const LEGACY_FILES: &[(&str, &str)] = &[(".cig-version", "cig"), (".cigar-version", "cigar")];

/// What the walk up from a directory found.
#[derive(Debug, Clone, Default)]
pub struct ProjectCtx {
    /// The directory holding the file that decided.
    pub root: PathBuf,
    /// A carton pin and the file it came from.
    pub carton_pin: Option<(Pin, PathBuf)>,
    /// Legacy per-tool pins (`tool`, pin, file).
    pub legacy: Vec<(String, Pin, PathBuf)>,
}

fn bad_pin_file(file: &Path, why: &str) -> InstallerError {
    InstallerError::new(
        "E1841",
        format!("cannot read the pin in {}: {why}", file.display()),
    )
    .with_fix("ciginstaller pin carton <pin>")
    .with_exit(EXIT_USAGE)
}

/// Walk up from `start` with the shim's own reader (`cigi_shim::find`: same files, same owner and
/// regular-file rules, same 40 levels, same stop at `user_home`), so `which` and the shim agree.
/// The installer's cap is larger than the shim's (the shim sends a bigger file here). A file that
/// is there and ours but unusable is a verdict (E1841), never skipped. `None` when nothing is
/// pinned anywhere.
pub fn discover(
    start: &Path,
    cigscript_home: &Path,
    user_home: Option<&Path>,
) -> Result<Option<ProjectCtx>, InstallerError> {
    let t = trusted_for(cigscript_home);
    let names: Vec<&str> = LEGACY_FILES.iter().map(|(n, _)| *n).collect();
    match find(start, &names, t, user_home, MAX_FILE_INSTALLER) {
        Found::Nothing => Ok(None),
        Found::Unusable { file, why, .. } => Err(bad_pin_file(&file, &why)),
        Found::Pin { dir, file, pin, .. } => {
            let pin = pin.ok_or_else(|| bad_pin_file(&file, "the file holds no pin"))?;
            let parsed = Pin::parse(&pin).map_err(|e| bad_pin_file(&file, &e.to_string()))?;
            Ok(Some(ProjectCtx {
                root: dir,
                carton_pin: Some((parsed, file)),
                legacy: Vec::new(),
            }))
        }
        Found::Legacy { dir, files } => {
            let mut legacy = Vec::new();
            for p in files {
                let name = file_label(&p);
                let Some((_, tool)) = LEGACY_FILES.iter().find(|(n, _)| *n == name) else {
                    continue;
                };
                let text = match read_capped(&p, t, MAX_FILE_INSTALLER) {
                    Reading::Absent => continue,
                    Reading::Unusable(why) => return Err(bad_pin_file(&p, &why)),
                    Reading::Text(s) => s,
                };
                let line = text
                    .lines()
                    .map(str::trim)
                    .find(|l| !l.is_empty() && !l.starts_with('#'))
                    .ok_or_else(|| bad_pin_file(&p, "the file holds no constraint"))?;
                let pin = Pin::parse(line).map_err(|e| bad_pin_file(&p, &e.to_string()))?;
                legacy.push((tool.to_string(), pin, p));
            }
            Ok((!legacy.is_empty()).then_some(ProjectCtx {
                root: dir,
                carton_pin: None,
                legacy,
            }))
        }
    }
}

/// Where `pin carton` writes: the nearest directory upward with `cig-tasks.json`, `cigar.toml` or
/// `.git`, else `start`. The same stop at `user_home` as the shim's walk, and a marker counts only
/// when it is not a link and is owned by the home's owner or root.
pub fn project_root(start: &Path, cigscript_home: &Path, user_home: Option<&Path>) -> PathBuf {
    let t = trusted_for(cigscript_home);
    let stop = user_home.filter(|h| start.starts_with(h));
    let mut dir = Some(start);
    for _ in 0..MAX_DEPTH {
        let Some(d) = dir else { break };
        if trusted_entry(&d.join("cig-tasks.json"), t, false)
            || trusted_entry(&d.join("cigar.toml"), t, false)
            || trusted_entry(&d.join(".git"), t, true)
        {
            return d.to_path_buf();
        }
        if stop == Some(d) {
            break;
        }
        dir = d.parent();
    }
    start.to_path_buf()
}

/// The name of a pin file for messages (`.cig-carton`), not its whole path.
pub fn file_label(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}
