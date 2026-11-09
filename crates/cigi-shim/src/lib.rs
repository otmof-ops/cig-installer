// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0

//! The one reader of project pin files (SPEC sections 6.5 and 8), shared by the shim and by
//! `ciginstaller`, so the fast path and the resolver cannot disagree about which file counts.
//! Std only, no dependencies (decision D6). It never writes.
//!
//! The rules, for every pin file (the three current ones and the legacy per-tool ones):
//! only a regular file (never a link, fifo, socket or device: checked before opening, and again
//! on the opened file), owned by the owner of `CIGSCRIPT_HOME` or by root. Anything else is the
//! same as no file. The walk climbs at most 40 levels and stops at `$HOME` when it starts inside it.
#![forbid(unsafe_code)]

use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

pub mod pins;

use pins::{pin_from_carton_file, pin_from_json, pin_from_toml};

/// The tools the shim serves.
pub const TOOLS: [&str; 4] = ["cig", "cigo", "cigcompile", "cigar"];
/// The most levels one walk climbs.
pub const MAX_DEPTH: usize = 40;
/// The shim's cap on a pin file. A larger one goes to the installer, which takes `MAX_FILE_INSTALLER`.
pub const MAX_FILE: u64 = 64 * 1024;
/// The installer's cap on a pin file.
pub const MAX_FILE_INSTALLER: u64 = 1024 * 1024;

/// Who may own a project pin file: the owner of CIGSCRIPT_HOME, or root. `None` off Unix.
#[derive(Debug, Clone, Copy, Default)]
pub struct Trusted {
    uid: Option<u32>,
    /// The check is on (Unix); a home whose owner cannot be read then trusts no pin file.
    on: bool,
}

#[cfg(unix)]
pub fn trusted_for(home: &Path) -> Trusted {
    use std::os::unix::fs::MetadataExt;
    Trusted {
        uid: fs::metadata(home).ok().map(|m| m.uid()),
        on: true,
    }
}
#[cfg(not(unix))]
pub fn trusted_for(_home: &Path) -> Trusted {
    Trusted::default()
}

#[cfg(unix)]
fn owner_ok(md: &fs::Metadata, t: Trusted) -> bool {
    use std::os::unix::fs::MetadataExt;
    !t.on || md.uid() == 0 || Some(md.uid()) == t.uid
}
#[cfg(not(unix))]
fn owner_ok(_md: &fs::Metadata, _t: Trusted) -> bool {
    true
}

/// The user's home directory as the shim sees it: `$HOME`, else `$USERPROFILE`.
pub fn user_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// The outcome of reading one pin file.
#[derive(Debug, PartialEq)]
pub enum Reading {
    /// No such file, or not one we may trust (a link, another owner, not a regular file):
    /// the same as no file.
    Absent,
    Text(String),
    /// It exists and is ours to read, but not within the cap, not text, or not readable. The text says why.
    Unusable(String),
}

/// Is `path` something we may trust without opening it: not a link, owned by a trusted user, and a
/// regular file (or a directory when `dir_ok`)? Never opens it.
pub fn trusted_entry(path: &Path, t: Trusted, dir_ok: bool) -> bool {
    fs::symlink_metadata(path).is_ok_and(|md| {
        let ft = md.file_type();
        (ft.is_file() || (dir_ok && ft.is_dir())) && owner_ok(&md, t)
    })
}

/// Read a pin file: only a regular file (never a link, fifo or device, checked before opening and
/// again on the opened file), owned by a trusted user, at most `cap` bytes of UTF-8.
pub fn read_capped(path: &Path, t: Trusted, cap: u64) -> Reading {
    let Ok(before) = fs::symlink_metadata(path) else {
        return Reading::Absent;
    };
    if !before.file_type().is_file() || !owner_ok(&before, t) {
        return Reading::Absent;
    }
    let f = match File::open(path) {
        Ok(f) => f,
        Err(e) => return Reading::Unusable(format!("it cannot be opened: {e}")),
    };
    let Ok(after) = f.metadata() else {
        return Reading::Unusable("it cannot be inspected".to_string());
    };
    if !after.is_file() || !owner_ok(&after, t) {
        return Reading::Absent;
    }
    // one byte past the cap tells "exactly at the cap" from "over it"
    let mut bytes = Vec::new();
    match f.take(cap + 1).read_to_end(&mut bytes) {
        Err(e) => return Reading::Unusable(format!("it cannot be read: {e}")),
        Ok(_) if bytes.len() as u64 > cap => {
            return Reading::Unusable(format!("it is larger than {cap} bytes"))
        }
        Ok(_) => {}
    }
    String::from_utf8(bytes).map_or_else(
        |_| Reading::Unusable("it is not UTF-8 text".to_string()),
        Reading::Text,
    )
}

/// Which dialect a pin file speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    CartonFile,
    Json,
    Toml,
}

/// What the walk up from a directory found.
#[derive(Debug, PartialEq)]
pub enum Found {
    /// A current pin file decided. `pin` is `None` only for a `.cig-carton` that holds no pin
    /// (a verdict by itself: the resolver says so).
    Pin {
        dir: PathBuf,
        file: PathBuf,
        kind: Kind,
        pin: Option<String>,
    },
    /// No current pin file, but these legacy per-tool files are here and trusted.
    Legacy {
        dir: PathBuf,
        files: Vec<PathBuf>,
    },
    /// A pin file is there and ours but unusable here (over the cap, not UTF-8, unreadable).
    Unusable {
        dir: PathBuf,
        file: PathBuf,
        why: String,
    },
    Nothing,
}

/// Walk up from `start` (at most 40 levels): the first directory with a verdict decides.
/// In one directory: `.cig-carton`, then `cig-tasks.json`, then `cigar.toml`, then the `legacy` names.
///
/// `stop`: the user's home directory. A project inside it is searched up to and including the home
/// and no further, so `/home/.cig-carton` or `/tmp/.cig-carton` never reaches into it.
pub fn find(start: &Path, legacy: &[&str], t: Trusted, stop: Option<&Path>, cap: u64) -> Found {
    let stop = stop.filter(|h| start.starts_with(h));
    let mut dir = Some(start);
    for _ in 0..MAX_DEPTH {
        let Some(d) = dir else { break };
        for (name, kind) in [
            (".cig-carton", Kind::CartonFile),
            ("cig-tasks.json", Kind::Json),
            ("cigar.toml", Kind::Toml),
        ] {
            let file = d.join(name);
            match read_capped(&file, t, cap) {
                Reading::Absent => {}
                Reading::Unusable(why) => {
                    return Found::Unusable {
                        dir: d.to_path_buf(),
                        file,
                        why,
                    }
                }
                Reading::Text(text) => {
                    let pin = match kind {
                        Kind::CartonFile => pin_from_carton_file(&text),
                        Kind::Json => pin_from_json(&text),
                        Kind::Toml => pin_from_toml(&text),
                    };
                    // a `.cig-carton` is a verdict by itself; the others only when they pin
                    if kind == Kind::CartonFile || pin.is_some() {
                        return Found::Pin {
                            dir: d.to_path_buf(),
                            file,
                            kind,
                            pin,
                        };
                    }
                }
            }
        }
        let files: Vec<PathBuf> = legacy
            .iter()
            .map(|n| d.join(n))
            .filter(|p| trusted_entry(p, t, false))
            .collect();
        if !files.is_empty() {
            return Found::Legacy {
                dir: d.to_path_buf(),
                files,
            };
        }
        if stop == Some(d) {
            break;
        }
        dir = d.parent();
    }
    Found::Nothing
}

#[cfg(test)]
mod lib_tests;
