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
    /// PowerShell 7 (`pwsh`).
    PowerShell,
    /// Windows PowerShell 5.1 (`powershell.exe`): its own profile, beside the 7 one.
    WindowsPowerShell,
}

impl Shell {
    /// The shells whose profile a PATH block goes into by default on this platform: both
    /// PowerShells on Windows (what `cmd.exe` users get is the registry entry), one shell
    /// elsewhere, picked by the caller.
    pub fn windows_defaults() -> [Shell; 2] {
        [Shell::PowerShell, Shell::WindowsPowerShell]
    }

    /// From a `$SHELL` value or a program name (`/bin/zsh`, `pwsh.exe`).
    pub fn from_program(s: &str) -> Option<Shell> {
        let base = s.rsplit(['/', '\\']).next().unwrap_or(s);
        let base = base.strip_suffix(".exe").unwrap_or(base);
        match base {
            "sh" | "dash" | "ash" => Some(Shell::Sh),
            "bash" => Some(Shell::Bash),
            "zsh" => Some(Shell::Zsh),
            "fish" => Some(Shell::Fish),
            "pwsh" => Some(Shell::PowerShell),
            "powershell" | "windowspowershell" => Some(Shell::WindowsPowerShell),
            _ => None,
        }
    }
}

/// What an add or remove did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathChange {
    Added,
    Removed,
    /// Remove: the block went, and the profile is a link (or lies outside the user's home) and
    /// held nothing else: it is left as an empty file, because it is not the installer's to delete.
    Emptied,
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

/// The folder the PowerShell profiles live under. On Windows, for the real user's home, it is the
/// Documents known folder, so a OneDrive-redirected or moved Documents is honored; for any other
/// home (a test's, a `CIGSCRIPT_HOME` on another user's tree) and on other platforms it is
/// `<home>/Documents`.
pub fn documents_dir(home: &Home) -> PathBuf {
    #[cfg(windows)]
    {
        let real = std::env::var_os("USERPROFILE")
            .is_some_and(|p| same_entry(&p.to_string_lossy(), &home.user_home().to_string_lossy()));
        if real {
            if let Ok(d) = cigi_sys::env::documents_dir() {
                return d;
            }
        }
    }
    home.user_home().join("Documents")
}

/// Whether a profile is in the user's own tree: under the home, or under the Documents folder
/// the profiles were placed in, which OneDrive can put outside the home. A file there that the
/// add created is the installer's to delete.
fn in_users_tree(profile: &Path, home: &Home) -> bool {
    profile.starts_with(home.user_home()) || profile.starts_with(documents_dir(home))
}

/// Whether PowerShell's `$HOME` is the home this install used. On Windows `$HOME` is the
/// profile directory (`USERPROFILE`), whatever `HOME` says here, so a different home (a test's, a
/// `HOME` that is set elsewhere) must be written out in full, as for any other non-default home.
/// Elsewhere `$HOME` is `HOME`, the variable the home came from.
fn powershell_home_is_this_home(home: &Home) -> bool {
    if !cfg!(windows) {
        return true;
    }
    std::env::var_os("USERPROFILE")
        .is_some_and(|p| same_entry(&p.to_string_lossy(), &home.user_home().to_string_lossy()))
}

/// The line a PowerShell profile runs. `var_ok`: the bin is `$HOME/.cigscript/bin` and `$HOME` is
/// the home this install used; otherwise the directory is named in full.
fn powershell_body(windows: bool, var_ok: bool, bin: &str) -> String {
    let quoted = format!("'{}'", bin.replace('\'', "''"));
    if windows {
        // a nested shell inherits the registry entry already: do not stack a second copy
        let q = if var_ok {
            "\"$HOME\\.cigscript\\bin\"".to_string()
        } else {
            quoted
        };
        format!(
            "if (($env:PATH -split ';') -notcontains {q}) {{ $env:PATH = {q} + ';' + $env:PATH }}"
        )
    } else if var_ok {
        "$env:PATH = \"$HOME/.cigscript/bin:$env:PATH\"".to_string()
    } else {
        format!("$env:PATH = '{}:' + $env:PATH", bin.replace('\'', "''"))
    }
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
        Shell::PowerShell | Shell::WindowsPowerShell => {
            let var_ok = uses_home_var && powershell_home_is_this_home(home);
            let body = powershell_body(cfg!(windows), var_ok, &bin_s);
            let file = if cfg!(windows) || shell == Shell::WindowsPowerShell {
                let dir_name = if shell == Shell::WindowsPowerShell {
                    "WindowsPowerShell"
                } else {
                    "PowerShell"
                };
                documents_dir(home)
                    .join(dir_name)
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
            let mode = platform::mode_of(&profile);
            if rest.is_empty() && !separator {
                // delete a profile the add created; a link, or a file outside the user's home
                // (a dotfiles repository), is not ours to delete: write it back empty
                let is_link = fs::symlink_metadata(&link).is_ok_and(|m| m.file_type().is_symlink());
                if is_link || !in_users_tree(&profile, home) {
                    txn.write_block(&profile, MARKER, rest.as_bytes(), mode)?;
                    return Ok(PathChange::Emptied);
                }
                txn.delete_user_file(&profile)?;
            } else {
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

// ------------------------------------------------------------------ the user PATH in the registry

/// What the registry `Path` value is called in what the user is shown.
pub const REGISTRY_PATH: &str = r"HKCU\Environment\Path";

/// What an add or remove of the registry entry did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryChange {
    Added,
    /// Add: the entry is there already (the user's own, or a rerun).
    AlreadyPresent,
    Removed,
    /// Remove: the entry is not in the value.
    NotPresent,
    /// Not Windows: there is no registry.
    Unsupported,
}

/// The line shown before the registry is written.
pub fn registry_line(entry: &Path) -> String {
    format!("{REGISTRY_PATH}: add {}", entry.display())
}

/// Whether this run puts the directory on the user's registry PATH: on Windows, always in a
/// release build (`--no-path` is how a person says no). Test builds honor
/// `CIGI_NO_REGISTRY_PATH`, because the fixture's homes are not the user's and their installs must
/// leave the runner's registry alone unless a test asks for it.
pub fn registry_supported() -> bool {
    cfg!(windows) && !test_build_skips_registry()
}

/// Test builds only (the `fault-injection` feature, which a release build refuses to compile).
#[cfg(feature = "fault-injection")]
fn test_build_skips_registry() -> bool {
    std::env::var_os("CIGI_NO_REGISTRY_PATH").is_some_and(|v| !v.is_empty())
}

#[cfg(not(feature = "fault-injection"))]
fn test_build_skips_registry() -> bool {
    false
}

/// An entry as `PATH` compares it: trimmed, unquoted, `/` as `\`, no trailing separator, and
/// case folded (Windows paths are case-insensitive). The text is not expanded.
fn norm_entry(e: &str) -> String {
    let t = e.trim().trim_matches('"').replace('/', "\\");
    t.trim_end_matches('\\').to_lowercase()
}

/// Whether two `PATH` entries name the same directory, ignoring case and a trailing `\`.
pub fn same_entry(a: &str, b: &str) -> bool {
    let (a, b) = (norm_entry(a), norm_entry(b));
    !a.is_empty() && a == b
}

/// Whether a `PATH` value (`;`-separated) holds the entry.
pub fn path_value_has(value: &str, entry: &str) -> bool {
    value.split(';').any(|e| same_entry(e, entry))
}

/// The value with the entry put first; `None` when it is there already. Every other byte of the
/// value is kept as it was, so an expandable `%VAR%` entry stays one.
pub fn path_value_prepend(value: &str, entry: &str) -> Option<String> {
    if path_value_has(value, entry) {
        return None;
    }
    Some(if value.is_empty() {
        entry.to_string()
    } else {
        format!("{entry};{value}")
    })
}

/// The value without the first matching entry; `None` when it is not there. Nothing else in the
/// value changes: the rest keeps its order, spelling and separators.
pub fn path_value_remove(value: &str, entry: &str) -> Option<String> {
    let parts: Vec<&str> = value.split(';').collect();
    let at = parts.iter().position(|e| same_entry(e, entry))?;
    let rest: Vec<&str> = parts
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != at)
        .map(|(_, e)| *e)
        .collect();
    Some(rest.join(";"))
}

#[cfg(windows)]
fn registry_io(source: std::io::Error) -> PathError {
    PathError::Io {
        path: PathBuf::from(REGISTRY_PATH),
        source,
    }
}

/// Whether the user's registry `Path` holds the entry (`false` off Windows).
pub fn registry_path_has(entry: &Path) -> Result<bool, PathError> {
    #[cfg(windows)]
    {
        let cur = cigi_sys::env::read_user_path().map_err(registry_io)?;
        Ok(cur.is_some_and(|c| path_value_has(&c.text, &entry.to_string_lossy())))
    }
    #[cfg(not(windows))]
    {
        let _ = entry;
        Ok(false)
    }
}

/// Put the entry first in the user's registry `Path`, keeping the value's type (a missing value
/// is made `REG_EXPAND_SZ`), then broadcast the change. Idempotent. Not journaled: the caller
/// runs it after the commit and records what it added in `state.json`.
pub fn registry_path_add(entry: &Path) -> Result<RegistryChange, PathError> {
    #[cfg(windows)]
    {
        use cigi_sys::env::{read_user_path, write_user_path, PathKind};
        let cur = read_user_path().map_err(registry_io)?;
        let (kind, text) = match &cur {
            Some(c) => (c.kind, c.text.clone()),
            None => (PathKind::ExpandSz, String::new()),
        };
        let Some(next) = path_value_prepend(&text, &entry.to_string_lossy()) else {
            return Ok(RegistryChange::AlreadyPresent);
        };
        write_user_path(kind, &next).map_err(registry_io)?;
        cigi_sys::env::broadcast_environment_change();
        Ok(RegistryChange::Added)
    }
    #[cfg(not(windows))]
    {
        let _ = entry;
        Ok(RegistryChange::Unsupported)
    }
}

/// Take exactly the entry out of the user's registry `Path` (the value is deleted if that leaves
/// it empty, as it was absent before when the install made it), then broadcast.
pub fn registry_path_remove(entry: &Path) -> Result<RegistryChange, PathError> {
    #[cfg(windows)]
    {
        use cigi_sys::env::{delete_user_path, read_user_path, write_user_path};
        let Some(cur) = read_user_path().map_err(registry_io)? else {
            return Ok(RegistryChange::NotPresent);
        };
        let Some(next) = path_value_remove(&cur.text, &entry.to_string_lossy()) else {
            return Ok(RegistryChange::NotPresent);
        };
        if next.trim().is_empty() {
            delete_user_path().map_err(registry_io)?;
        } else {
            write_user_path(cur.kind, &next).map_err(registry_io)?;
        }
        cigi_sys::env::broadcast_environment_change();
        Ok(RegistryChange::Removed)
    }
    #[cfg(not(windows))]
    {
        let _ = entry;
        Ok(RegistryChange::Unsupported)
    }
}

#[cfg(test)]
mod registry_value_tests {
    use super::*;

    #[test]
    fn the_windows_powershell_block_names_a_non_default_home_in_full() {
        let bin = r"D:\cig\home\.cigscript\bin";
        let full = powershell_body(true, false, bin);
        assert!(full.contains(&format!("'{bin}'")), "{full}");
        assert!(!full.contains("$HOME"), "{full}");
        let var = powershell_body(true, true, r"C:\Users\me\.cigscript\bin");
        assert!(var.contains(r#""$HOME\.cigscript\bin""#), "{var}");
        // a quote in the path is doubled, not left to end the string early
        assert!(powershell_body(true, false, r"C:\it's\bin").contains(r"'C:\it''s\bin'"));
    }

    #[test]
    fn entries_compare_without_case_or_trailing_separator() {
        assert!(same_entry(
            r"C:\Users\Me\.cigscript\bin",
            r"c:\users\me\.CIGSCRIPT\bin\"
        ));
        assert!(same_entry(r#""C:\a\b""#, r"C:\a\b"));
        assert!(same_entry("C:/a/b", r"C:\a\b"));
        assert!(!same_entry(r"C:\a\b", r"C:\a\bc"));
        assert!(!same_entry("", ""));
    }

    #[test]
    fn prepend_keeps_the_rest_byte_for_byte() {
        let v = r"%USERPROFILE%\bin;C:\Tools\;;D:\x";
        let n = path_value_prepend(v, r"C:\h\bin").unwrap();
        assert_eq!(n, format!(r"C:\h\bin;{v}"));
        assert_eq!(path_value_prepend(&n, r"c:\H\BIN\"), None);
        assert_eq!(path_value_prepend("", r"C:\h\bin").unwrap(), r"C:\h\bin");
    }

    #[test]
    fn remove_restores_what_prepend_changed() {
        for v in ["", r"C:\a", r"%USERPROFILE%\bin;C:\Tools\;;D:\x", r"C:\a;"] {
            let added = path_value_prepend(v, r"C:\h\bin").unwrap();
            let back = path_value_remove(&added, r"C:\h\bin").unwrap();
            assert_eq!(back, v, "value {v:?}");
        }
        assert_eq!(path_value_remove(r"C:\a;C:\b", r"C:\h\bin"), None);
    }

    #[test]
    fn remove_takes_only_the_first_match() {
        let v = r"C:\h\bin;C:\a;c:\H\bin\";
        assert_eq!(
            path_value_remove(v, r"C:\h\bin").unwrap(),
            r"C:\a;c:\H\bin\"
        );
    }

    #[test]
    fn the_registry_line_names_the_value_and_the_directory() {
        assert_eq!(
            registry_line(Path::new(r"C:\h\bin")),
            r"HKCU\Environment\Path: add C:\h\bin"
        );
    }
}

// ------------------------------------------------------------------ what install records

/// The `state.json` key (in `extra`) for the profile blocks the install agreed to.
pub const PATH_BLOCK_KEY: &str = "path_block";
/// The `state.json` key (in `extra`) for the registry entry: `{ "entry", "preexisting" }`.
pub const PATH_REGISTRY_KEY: &str = "path_registry";

/// What `state.json` says about the registry entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryRecord {
    /// The directory put on the user's `Path`.
    pub entry: PathBuf,
    /// It was on the user's `Path` before the install: not ours, so uninstall leaves it.
    pub preexisting: bool,
}

/// The record for `state.json`. It is written in the install's own state save, before the
/// registry is touched, so a failed registry write is a record `doctor` can see and `repair` can
/// act on, never a half-applied transaction.
pub fn registry_record(entry: &Path, preexisting: bool) -> serde_json::Value {
    serde_json::json!({ "entry": entry.display().to_string(), "preexisting": preexisting })
}

/// The registry entry the install recorded.
pub fn recorded_registry(state: &crate::state::State) -> Option<RegistryRecord> {
    let v = state.extra.get(PATH_REGISTRY_KEY)?;
    Some(RegistryRecord {
        entry: PathBuf::from(v.get("entry")?.as_str()?),
        preexisting: v
            .get("preexisting")
            .and_then(|p| p.as_bool())
            .unwrap_or(true),
    })
}

/// The profile block record for `state.json`: the first shell as `shell` (what older builds
/// wrote and read), all of them as `shells`.
pub fn blocks_record(shells: &[Shell]) -> serde_json::Value {
    let name = |s: &Shell| format!("{s:?}").to_lowercase();
    serde_json::json!({
        "shell": shells.first().map(name),
        "shells": shells.iter().map(name).collect::<Vec<_>>(),
    })
}

/// The shells `install` added a PATH block for, when `state.json` says it did.
pub fn recorded_shells(state: &crate::state::State) -> Vec<Shell> {
    let Some(v) = state.extra.get(PATH_BLOCK_KEY) else {
        return Vec::new();
    };
    let mut out: Vec<Shell> = Vec::new();
    let named = v.get("shells").and_then(|s| s.as_array());
    let names: Vec<&str> = match named {
        Some(a) => a.iter().filter_map(|x| x.as_str()).collect(),
        None => v
            .get("shell")
            .and_then(|s| s.as_str())
            .into_iter()
            .collect(),
    };
    for n in names {
        if let Some(s) = Shell::from_program(n) {
            if !out.contains(&s) {
                out.push(s);
            }
        }
    }
    out
}
