// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! The shims in `bin/` (section 4): hard links of the `cigi-shim` object.

use std::fs;
use std::path::PathBuf;

use crate::error::ShimError;
use crate::home::Home;
use crate::journal::Txn;
use crate::platform;
use crate::store::ObjectId;

/// `bin/<name>` (with `.exe` on Windows).
pub fn shim_path(home: &Home, name: &str) -> PathBuf {
    home.bin().join(format!("{name}{}", platform::exe_suffix()))
}

/// A name that may become a file in `bin/`: `[a-z][a-z0-9-]*`. Names reach here from manifests
/// and from receipts on disk, so they are checked again before any file is made or removed.
pub fn shim_name_ok(name: &str) -> bool {
    let mut c = name.chars();
    matches!(c.next(), Some('a'..='z'))
        && c.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn checked_path(home: &Home, name: &str) -> Result<PathBuf, ShimError> {
    if !shim_name_ok(name) {
        return Err(ShimError::BadName(name.to_string()));
    }
    Ok(shim_path(home, name))
}

/// Make every `names` shim a hard link of `shim_object` (copy when linking is
/// impossible). A shim that already is the same file is left alone; a
/// different file (an older shim, a stray) is replaced by rename, its old
/// bytes saved in the journal. Also used for `bin/ciginstaller` and
/// `bin/cigi` with the installer's object.
pub fn sync_shims(
    txn: &mut Txn,
    home: &Home,
    names: &[&str],
    shim_object: &ObjectId,
) -> Result<(), ShimError> {
    let store = home.object_store();
    store
        .verify(shim_object)
        .map_err(|e| ShimError::Journal(e.into()))?;
    let obj = store.object_path(shim_object);
    txn.mkdir_all(&home.bin())?;
    for name in names {
        txn.link(&obj, &checked_path(home, name)?)?;
    }
    Ok(())
}

/// Remove the shims for `names` (journaled); absent ones are skipped.
pub fn remove_shims(txn: &mut Txn, home: &Home, names: &[&str]) -> Result<(), ShimError> {
    for name in names {
        txn.delete(&checked_path(home, name)?)?;
    }
    Ok(())
}

/// Shim names present in `bin/`: files that share an inode with `shim_object`
/// (or all regular files when the platform has no inodes and `names` match).
pub fn shims_present(home: &Home, shim_object: &ObjectId, names: &[&str]) -> Vec<String> {
    let obj = home.object_store().object_path(shim_object);
    let mut out = Vec::new();
    for n in names {
        if !shim_name_ok(n) {
            continue;
        }
        let p = shim_path(home, n);
        if !fs::metadata(&p).map(|m| m.is_file()).unwrap_or(false) {
            continue;
        }
        match (platform::ino(&p), platform::ino(&obj)) {
            (Some(a), Some(b)) if a != b => {}
            _ => out.push(n.to_string()),
        }
    }
    out
}
