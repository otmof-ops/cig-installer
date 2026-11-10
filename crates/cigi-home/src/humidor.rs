// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! The humidor mirror (section 4.4): cig's own shelf, `humidor/<ver>/cig` and
//! `humidor/index.json` (`{"entries": [...]}`, as `cigscript::humidor` reads it).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

use cig_compat::Version;

use crate::error::{HumidorError, ToolchainError};
use crate::home::Home;
use crate::journal::Txn;
use crate::platform;
use crate::toolchain;
use crate::util;

/// `source` value of entries this installer owns.
pub const SOURCE: &str = "ciginstaller";

/// What `mirror_humidor` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HumidorMirror {
    Mirrored,
    /// An entry or binary that is not ours stands there: left alone.
    Skipped,
}

/// One entry of `index.json` (cig's `humidor::Entry`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HumidorEntry {
    pub version: String,
    pub source: String,
    pub sha256: String,
    pub fetched_at: String,
    pub last_lit: Option<String>,
    pub trust: String,
}

fn index_path(home: &Home) -> PathBuf {
    home.humidor().join("index.json")
}

fn binary_name() -> String {
    format!("cig{}", platform::exe_suffix())
}

fn io(p: &Path) -> impl Fn(std::io::Error) -> HumidorError + '_ {
    move |source| HumidorError::Io {
        path: p.to_path_buf(),
        source,
    }
}

/// The index: `entries` plus whatever else the file holds, kept as it was.
struct Index {
    rest: serde_json::Map<String, Value>,
    entries: Vec<Value>,
}

impl Index {
    fn to_value(&self) -> Value {
        let mut m = self.rest.clone();
        m.insert("entries".to_string(), Value::Array(self.entries.clone()));
        Value::Object(m)
    }
}

fn load_index(home: &Home) -> Result<Index, HumidorError> {
    let p = index_path(home);
    let Some(text) = util::read_opt(&p).map_err(io(&p))? else {
        return Ok(Index {
            rest: serde_json::Map::new(),
            entries: Vec::new(),
        });
    };
    let v: Value = serde_json::from_str(&text).map_err(|e| HumidorError::BadIndex {
        path: p.clone(),
        why: e.to_string(),
    })?;
    let Value::Object(mut rest) = v else {
        return Err(HumidorError::BadIndex {
            path: p,
            why: "not a JSON object".into(),
        });
    };
    let entries = match rest.remove("entries") {
        Some(Value::Array(a)) => a,
        _ => Vec::new(),
    };
    Ok(Index { rest, entries })
}

fn find(entries: &[Value], version: &str) -> Option<usize> {
    entries
        .iter()
        .position(|e| e.get("version").and_then(|x| x.as_str()) == Some(version))
}

/// The entries of the index, read-only (for `doctor`).
pub fn entries(home: &Home) -> Result<Vec<HumidorEntry>, HumidorError> {
    Ok(load_index(home)?
        .entries
        .iter()
        .filter_map(|e| serde_json::from_value::<HumidorEntry>(e.clone()).ok())
        .collect())
}

fn write_index(txn: &mut Txn, home: &Home, idx: &Index) -> Result<(), HumidorError> {
    let mut text = serde_json::to_string_pretty(&idx.to_value()).unwrap_or_default();
    text.push('\n');
    let p = index_path(home);
    let mode = platform::mode_of(&p);
    txn.write(&p, text.as_bytes(), mode.or(Some(0o644)))?;
    Ok(())
}

/// Hard link `binary` (the installed cig) to `humidor/<ver>/cig` and register
/// it. Journaled; idempotent (a re-run with the same binary changes nothing).
/// An entry whose `source` is not `ciginstaller`, or a binary with no entry,
/// is left alone (`Skipped`). The cig receipt's `humidor_mirror` is set to
/// `"mirrored"` or `"skipped"` in the same transaction.
pub fn mirror_humidor(
    txn: &mut Txn,
    home: &Home,
    cig: &Version,
    binary: &Path,
) -> Result<HumidorMirror, HumidorError> {
    mirror_inner(txn, home, cig, binary, false)
}

/// `repair`: as [`mirror_humidor`] for a version whose receipt says it was mirrored, so a link
/// left standing without its index entry is not mistaken for someone else's binary. An entry
/// that is not ours is still left alone.
pub fn remirror_humidor(
    txn: &mut Txn,
    home: &Home,
    cig: &Version,
    binary: &Path,
) -> Result<HumidorMirror, HumidorError> {
    mirror_inner(txn, home, cig, binary, true)
}

fn mirror_inner(
    txn: &mut Txn,
    home: &Home,
    cig: &Version,
    binary: &Path,
    ours_if_unlisted: bool,
) -> Result<HumidorMirror, HumidorError> {
    let ver = cig.to_string();
    let dir = home.humidor().join(&ver);
    let target = dir.join(binary_name());
    let mut index = load_index(home)?;
    let existing = find(&index.entries, &ver);
    let old: Option<Value> = existing.and_then(|i| index.entries.get(i).cloned());
    let ours = old
        .as_ref()
        .map(|e| e.get("source").and_then(|s| s.as_str()) == Some(SOURCE));
    let outcome = match (existing, ours) {
        (Some(_), Some(false)) => HumidorMirror::Skipped,
        (None, _) if util::exists_any(&target) && !ours_if_unlisted => HumidorMirror::Skipped,
        _ => HumidorMirror::Mirrored,
    };
    let receipt = toolchain::read_receipt(home, "cig", cig).ok();
    if outcome == HumidorMirror::Mirrored {
        let (sha256, _) = util::sha256_file(binary).map_err(io(binary))?;
        let trust = match receipt.as_ref().map(|r| r.trust.level.as_str()) {
            Some("attested") => "attested",
            _ => "verified",
        };
        txn.mkdir_all(&dir)?;
        txn.link(binary, &target)?;
        let mut entry = HumidorEntry {
            version: ver.clone(),
            source: SOURCE.to_string(),
            sha256,
            fetched_at: util::now_rfc3339(),
            last_lit: None,
            trust: trust.to_string(),
        };
        if let Some(prev) = old
            .filter(|_| ours == Some(true))
            .and_then(|e| serde_json::from_value::<HumidorEntry>(e).ok())
        {
            // Same binary again: keep what the entry already says.
            if prev.sha256 == entry.sha256 {
                entry.fetched_at = prev.fetched_at;
            }
            entry.last_lit = prev.last_lit;
        }
        let val = serde_json::to_value(&entry).unwrap_or(Value::Null);
        match existing {
            Some(i) => index.entries[i] = val,
            None => index.entries.push(val),
        }
        write_index(txn, home, &index)?;
    }
    if let Some(mut r) = receipt {
        let mark = match outcome {
            HumidorMirror::Mirrored => "mirrored",
            HumidorMirror::Skipped => "skipped",
        };
        if r.humidor_mirror.as_deref() != Some(mark) {
            r.humidor_mirror = Some(mark.to_string());
            toolchain::write_receipt(txn, home, &r).map_err(|e| match e {
                ToolchainError::Journal(j) => HumidorError::Journal(j),
                other => HumidorError::Receipt(other.to_string()),
            })?;
        }
    }
    Ok(outcome)
}

/// Remove our mirror of a cig version: its directory and its index entry.
/// An entry that is not ours is left alone. Returns whether anything went.
pub fn remove_humidor_mirror(
    txn: &mut Txn,
    home: &Home,
    cig: &Version,
) -> Result<bool, HumidorError> {
    let ver = cig.to_string();
    let mut index = load_index(home)?;
    let Some(i) = find(&index.entries, &ver) else {
        return Ok(false);
    };
    let is_ours = index.entries[i].get("source").and_then(|s| s.as_str()) == Some(SOURCE);
    if !is_ours {
        return Ok(false);
    }
    txn.delete(&home.humidor().join(&ver))?;
    index.entries.remove(i);
    write_index(txn, home, &index)?;
    Ok(true)
}

/// Remove every mirror this installer made: each `humidor/<ver>/` and its index entry, for
/// `uninstall --all`. Entries that are not ours stay, and so do the index and the directory when
/// anything else is in them. An index left with no entry and no other key is deleted, and the
/// `humidor/` directory with it when it is empty. Returns the versions removed.
pub fn remove_all_humidor_mirrors(txn: &mut Txn, home: &Home) -> Result<Vec<String>, HumidorError> {
    let mut index = load_index(home)?;
    let mut gone = Vec::new();
    let mut keep = Vec::new();
    for e in std::mem::take(&mut index.entries) {
        let ours = e.get("source").and_then(|s| s.as_str()) == Some(SOURCE);
        let ver = e
            .get("version")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        // the name of a directory under humidor/ is a version that parses and prints as it was
        // written: `.`, `..`, `a/b` and the like are never a path to delete
        let plain = ver
            .as_deref()
            .filter(|v| Version::parse(v).is_ok_and(|p| p.to_string() == *v));
        match (ours, plain) {
            (true, Some(v)) => {
                txn.delete(&home.humidor().join(v))?;
                gone.push(v.to_string());
            }
            _ => keep.push(e),
        }
    }
    if gone.is_empty() {
        return Ok(gone);
    }
    index.entries = keep;
    if index.entries.is_empty() && index.rest.is_empty() {
        txn.delete(&index_path(home))?;
        let empty = std::fs::read_dir(home.humidor())
            .map(|mut r| r.next().is_none())
            .unwrap_or(false);
        if empty {
            txn.delete(&home.humidor())?;
        }
    } else {
        write_index(txn, home, &index)?;
    }
    Ok(gone)
}
