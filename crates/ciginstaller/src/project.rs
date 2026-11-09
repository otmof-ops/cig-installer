// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0
//! The project a command runs in: the pin file the shim would see (SPEC 6.5), found the same way.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use cig_compat::Pin;
use cigi_ui::{InstallerError, EXIT_USAGE};

const MAX_DEPTH: usize = 40;
const MAX_FILE: u64 = 64 * 1024;

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

fn read_capped(p: &Path) -> Option<String> {
    let mut s = String::new();
    fs::File::open(p)
        .ok()?
        .take(MAX_FILE)
        .read_to_string(&mut s)
        .ok()?;
    Some(s)
}

fn bad_pin_file(file: &Path, why: &str) -> InstallerError {
    InstallerError::new(
        "E1841",
        format!("cannot read the pin in {}: {why}", file.display()),
    )
    .with_fix("ciginstaller pin carton <pin>")
    .with_exit(EXIT_USAGE)
}

/// `.cig-carton`: the first line that is not blank or a comment.
fn carton_file_pin(text: &str, file: &Path) -> Result<Pin, InstallerError> {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .ok_or_else(|| bad_pin_file(file, "the file holds no pin"))?;
    Pin::parse_carton_line(line).map_err(|e| bad_pin_file(file, &e.to_string()))
}

/// The string value of the top-level key `"carton"` of a JSON object (no JSON library: the shim does the same).
pub fn json_carton_key(text: &str) -> Option<String> {
    let b = text.as_bytes();
    let (mut i, mut depth) = (0, 0usize);
    while i < b.len() {
        match b[i] {
            b'{' | b'[' => depth += 1,
            b'}' | b']' => depth = depth.saturating_sub(1),
            b'"' => {
                let start = i + 1;
                i = start;
                while i < b.len() && b[i] != b'"' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                let key = text.get(start..i.min(b.len()))?;
                let mut j = i + 1;
                while j < b.len() && b[j].is_ascii_whitespace() {
                    j += 1;
                }
                if depth == 1 && key == "carton" && b.get(j) == Some(&b':') {
                    j += 1;
                    while j < b.len() && b[j].is_ascii_whitespace() {
                        j += 1;
                    }
                    if b.get(j) != Some(&b'"') {
                        return None;
                    }
                    let end = text[j + 1..].find('"')? + j + 1;
                    let v = &text[j + 1..end];
                    return (!v.contains('\\') && !v.is_empty()).then(|| v.to_string());
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// `carton = "0.1"` before the first table of a TOML file.
pub fn toml_carton_key(text: &str) -> Option<String> {
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            return None;
        }
        let Some(rest) = line.strip_prefix("carton") else {
            continue;
        };
        let Some(val) = rest.trim_start().strip_prefix('=') else {
            continue;
        };
        let val = val.trim();
        let q = val.chars().next().filter(|c| *c == '"' || *c == '\'')?;
        let inner = &val[1..];
        return inner
            .find(q)
            .map(|e| inner[..e].to_string())
            .filter(|s| !s.is_empty());
    }
    None
}

fn legacy_in(dir: &Path) -> Result<Vec<(String, Pin, PathBuf)>, InstallerError> {
    let mut out = Vec::new();
    for (file, tool) in LEGACY_FILES {
        let p = dir.join(file);
        let Some(text) = read_capped(&p) else {
            continue;
        };
        let line = text
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty() && !l.starts_with('#'))
            .ok_or_else(|| bad_pin_file(&p, "the file holds no constraint"))?;
        let pin = Pin::parse(line).map_err(|e| bad_pin_file(&p, &e.to_string()))?;
        out.push((tool.to_string(), pin, p));
    }
    Ok(out)
}

/// Walk up from `start` (at most 40 levels, to the filesystem root); the first directory with a
/// verdict decides, exactly as the shim does: `.cig-carton`, then `cig-tasks.json`, then
/// `cigar.toml`, then the legacy files. `None` when nothing is pinned anywhere.
pub fn discover(start: &Path) -> Result<Option<ProjectCtx>, InstallerError> {
    let mut dir = Some(start);
    for _ in 0..MAX_DEPTH {
        let Some(d) = dir else { break };
        let mut ctx = ProjectCtx {
            root: d.to_path_buf(),
            ..ProjectCtx::default()
        };
        let carton_file = d.join(".cig-carton");
        let tasks = d.join("cig-tasks.json");
        let cigar = d.join("cigar.toml");
        if let Some(t) = read_capped(&carton_file) {
            ctx.carton_pin = Some((carton_file_pin(&t, &carton_file)?, carton_file));
        } else if let Some(v) = read_capped(&tasks).and_then(|t| json_carton_key(&t)) {
            let pin = Pin::parse(&v).map_err(|e| bad_pin_file(&tasks, &e.to_string()))?;
            ctx.carton_pin = Some((pin, tasks));
        } else if let Some(v) = read_capped(&cigar).and_then(|t| toml_carton_key(&t)) {
            let pin = Pin::parse(&v).map_err(|e| bad_pin_file(&cigar, &e.to_string()))?;
            ctx.carton_pin = Some((pin, cigar));
        }
        ctx.legacy = legacy_in(d)?;
        if ctx.carton_pin.is_some() || !ctx.legacy.is_empty() {
            return Ok(Some(ctx));
        }
        dir = d.parent();
    }
    Ok(None)
}

/// Where `pin carton` writes: the nearest directory upward with `cig-tasks.json`, `cigar.toml` or `.git`, else `start`.
pub fn project_root(start: &Path) -> PathBuf {
    let mut dir = Some(start);
    for _ in 0..MAX_DEPTH {
        let Some(d) = dir else { break };
        if d.join("cig-tasks.json").is_file()
            || d.join("cigar.toml").is_file()
            || d.join(".git").exists()
        {
            return d.to_path_buf();
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
