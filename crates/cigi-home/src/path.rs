// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! The PATH block in a shell profile (section 12.1 step 5).

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::PathError;
use crate::home::Home;
use crate::platform;
use crate::util;

/// Marker name journaled with the `block` op.
pub const MARKER: &str = "cigscript";
/// First line of the block.
pub const BLOCK_BEGIN: &str = "# >>> cigscript >>>";
/// Last line of the block.
pub const BLOCK_END: &str = "# <<< cigscript <<<";

/// The user's shell, as far as profiles are concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    Sh,
    Bash,
    Zsh,
    Fish,
    PowerShell,
}

impl Shell {
    /// From a `$SHELL` value or a program name (`/bin/zsh`, `pwsh.exe`).
    pub fn from_program(s: &str) -> Option<Shell> {
        let base = s.rsplit(['/', '\\']).next().unwrap_or(s);
        let base = base.strip_suffix(".exe").unwrap_or(base);
        match base {
            "sh" | "dash" | "ash" => Some(Shell::Sh),
            "bash" => Some(Shell::Bash),
            "zsh" => Some(Shell::Zsh),
            "fish" => Some(Shell::Fish),
            "pwsh" | "powershell" => Some(Shell::PowerShell),
            _ => None,
        }
    }
}

/// What an add or remove did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathChange {
    Added,
    Removed,
    /// Add: the exact block is already there.
    AlreadyPresent,
    /// Remove: no block.
    NotPresent,
    /// The markers are there but the content is not ours (hand-edited, or a
    /// marker without its partner): left alone, `E1862`.
    Edited,
    /// The profile is a symbolic link that leads nowhere usable (dangling, or
    /// not to a file): left alone, the caller shows the block to add by hand.
    Skipped,
}

/// Where a profile really lives. A symbolic link (a dotfiles repository, say)
/// is followed: the block goes into the target, and the link stays a link.
/// `None` when the link cannot be followed to a regular file.
fn resolve_profile(p: &Path) -> Option<PathBuf> {
    match fs::symlink_metadata(p) {
        Ok(md) if md.file_type().is_symlink() => match fs::canonicalize(p) {
            Ok(t) if t.is_file() => Some(t),
            _ => None,
        },
        _ => Some(p.to_path_buf()),
    }
}

fn dq_escape(s: &str) -> String {
    let mut o = String::new();
    for c in s.chars() {
        if matches!(c, '\\' | '"' | '$' | '`') {
            o.push('\\');
        }
        o.push(c);
    }
    o
}

fn sq_escape_fish(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

/// `(profile file, block text)` for a shell. The block ends with a newline.
pub fn path_block(shell: Shell, home: &Home) -> (PathBuf, String) {
    let user = home.user_home();
    let bin = home.bin();
    let default_bin = user.join(".cigscript").join("bin");
    let uses_home_var = bin == default_bin;
    let bin_s = bin.display().to_string();
    let (profile, body) = match shell {
        Shell::Sh | Shell::Bash | Shell::Zsh => {
            let file = if shell == Shell::Zsh {
                ".zprofile"
            } else {
                ".profile"
            };
            let dir = if uses_home_var {
                "$HOME/.cigscript/bin".to_string()
            } else {
                dq_escape(&bin_s)
            };
            (user.join(file), format!("export PATH=\"{dir}:$PATH\""))
        }
        Shell::Fish => {
            let body = if uses_home_var {
                "fish_add_path \"$HOME/.cigscript/bin\"".to_string()
            } else {
                format!("fish_add_path '{}'", sq_escape_fish(&bin_s))
            };
            (
                user.join(".config")
                    .join("fish")
                    .join("conf.d")
                    .join("cigscript.fish"),
                body,
            )
        }
        Shell::PowerShell => {
            let sep = if cfg!(windows) { ';' } else { ':' };
            let body = if uses_home_var {
                if cfg!(windows) {
                    "$env:PATH = \"$HOME\\.cigscript\\bin;$env:PATH\"".to_string()
                } else {
                    "$env:PATH = \"$HOME/.cigscript/bin:$env:PATH\"".to_string()
                }
            } else {
                format!(
                    "$env:PATH = '{}{sep}' + $env:PATH",
                    bin_s.replace('\'', "''")
                )
            };
            let file = if cfg!(windows) {
                user.join("Documents")
                    .join("PowerShell")
                    .join("Microsoft.PowerShell_profile.ps1")
            } else {
                user.join(".config")
                    .join("powershell")
                    .join("Microsoft.PowerShell_profile.ps1")
            };
            (file, body)
        }
    };
    (profile, format!("{BLOCK_BEGIN}\n{body}\n{BLOCK_END}\n"))
}

enum Found {
    None,
    /// Byte range of the whole block, begin line through end line + newline.
    At(usize, usize),
    Broken,
}

fn find_block(text: &str) -> Found {
    let mut pos = 0;
    let mut begin = None;
    for line in text.split_inclusive('\n') {
        let t = line.trim_end_matches(['\n', '\r']);
        match (t, begin) {
            (BLOCK_BEGIN, None) => begin = Some(pos),
            (BLOCK_BEGIN, Some(_)) => return Found::Broken,
            (BLOCK_END, Some(b)) => {
                let end = pos + line.len();
                // A final line without a newline still ends the block.
                return Found::At(b, end);
            }
            (BLOCK_END, None) => return Found::Broken,
            _ => {}
        }
        pos += line.len();
    }
    if begin.is_some() {
        Found::Broken
    } else {
        Found::None
    }
}

fn read(p: &Path) -> Result<Option<String>, PathError> {
    util::read_opt(p).map_err(|source| PathError::Io {
        path: p.to_path_buf(),
        source,
    })
}

/// Add the block to the shell's profile, journaled as a `block` op.
/// Idempotent; an edited block is never touched.
pub fn add_path_block(
    txn: &mut crate::journal::Txn,
    shell: Shell,
    home: &Home,
) -> Result<PathChange, PathError> {
    let (link, block) = path_block(shell, home);
    let Some(profile) = resolve_profile(&link) else {
        return Ok(PathChange::Skipped);
    };
    let existing = read(&profile)?;
    let mut text = existing.clone().unwrap_or_default();
    let crlf = text.contains("\r\n");
    match find_block(&text) {
        Found::At(s, e) => {
            return Ok(if text[s..e].replace("\r\n", "\n") == block {
                PathChange::AlreadyPresent
            } else {
                PathChange::Edited
            })
        }
        Found::Broken => return Ok(PathChange::Edited),
        Found::None => {}
    }
    // A profile that exists always gets the separator, an empty one too, so
    // that removal can tell it from a file the add created. A CRLF profile
    // gets CRLF line ends.
    let nl = if crlf { "\r\n" } else { "\n" };
    if existing.is_some() {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push_str(nl);
        }
        text.push_str(nl);
    }
    if crlf {
        text.push_str(&block.replace('\n', "\r\n"));
    } else {
        text.push_str(&block);
    }
    if let Some(parent) = profile.parent() {
        txn.mkdir_all(parent)?;
    }
    let mode = if existing.is_some() {
        platform::mode_of(&profile)
    } else {
        None
    };
    txn.write_block(&profile, MARKER, text.as_bytes(), mode)?;
    Ok(PathChange::Added)
}

/// Remove the block, and the blank line `add_path_block` put before it. A
/// profile the add created (it held only the block) is deleted; one that
/// existed, an empty one included, goes back byte for byte, CRLF too, when it
/// ended with a newline before the add.
pub fn remove_path_block(
    txn: &mut crate::journal::Txn,
    shell: Shell,
    home: &Home,
) -> Result<PathChange, PathError> {
    let (link, block) = path_block(shell, home);
    let Some(profile) = resolve_profile(&link) else {
        return Ok(PathChange::Skipped);
    };
    let Some(text) = read(&profile)? else {
        return Ok(PathChange::NotPresent);
    };
    match find_block(&text) {
        Found::None => Ok(PathChange::NotPresent),
        Found::Broken => Ok(PathChange::Edited),
        Found::At(s, e) => {
            if text[s..e].replace("\r\n", "\n") != block {
                return Ok(PathChange::Edited);
            }
            let mut start = s;
            let mut separator = false;
            if start > 0 && text.as_bytes()[start - 1] == b'\n' {
                start -= 1;
                separator = true;
                if start > 0 && text.as_bytes()[start - 1] == b'\r' {
                    start -= 1;
                }
            }
            let rest = format!("{}{}", &text[..start], &text[e..]);
            if rest.is_empty() && !separator {
                txn.delete_user_file(&profile)?;
            } else {
                let mode = platform::mode_of(&profile);
                txn.write_block(&profile, MARKER, rest.as_bytes(), mode)?;
            }
            Ok(PathChange::Removed)
        }
    }
}

/// Whether the profile has the exact block (for `doctor`).
pub fn block_state(shell: Shell, home: &Home) -> Result<PathChange, PathError> {
    let (link, block) = path_block(shell, home);
    let Some(profile) = resolve_profile(&link) else {
        return Ok(PathChange::Skipped);
    };
    let Some(text) = read(&profile)? else {
        return Ok(PathChange::NotPresent);
    };
    Ok(match find_block(&text) {
        Found::None => PathChange::NotPresent,
        Found::Broken => PathChange::Edited,
        Found::At(s, e) => {
            if text[s..e].replace("\r\n", "\n") == block {
                PathChange::AlreadyPresent
            } else {
                PathChange::Edited
            }
        }
    })
}
