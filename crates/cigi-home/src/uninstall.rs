// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! The last step of `uninstall --all` (SPEC 12.8): `installer/` goes aside, whole, and its rename
//! is the commit point.
//!
//! Everything before it is journaled and undone by `recover` when the process dies (no `flip`
//! line: roll back). The rename moves `installer/`, and the open journal with it, to a sibling of
//! the home named `<home>-uninstall-<txn id>` (`~/.cigscript-uninstall-<id>` for the default
//! home). Before the rename the home is whole; after it, `installer/` is gone, and nothing is left
//! to recover. The only paths this module renames or deletes are `<home>/installer` and that
//! sibling name; it accepts no other.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::JournalError;
use crate::home::Home;
use crate::journal::{fault_after, read_journal, JournalOp, Txn};
use crate::platform;

/// The marker in the sibling's name.
const INFIX: &str = "-uninstall-";

fn misuse(why: impl Into<String>) -> JournalError {
    JournalError::Misuse(why.into())
}

fn io(p: &Path) -> impl Fn(std::io::Error) -> JournalError + '_ {
    move |source| JournalError::Io {
        path: p.to_path_buf(),
        source,
    }
}

/// `YYYYMMDDTHHMMSS-<6 hex>`, the shape of a transaction id.
fn plain_id(id: &str) -> bool {
    let b = id.as_bytes();
    b.len() == 22
        && b[..8].iter().all(u8::is_ascii_digit)
        && b[8] == b'T'
        && b[9..15].iter().all(u8::is_ascii_digit)
        && b[15] == b'-'
        && b[16..]
            .iter()
            .all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
}

fn root_name(home: &Home) -> Option<String> {
    home.root()
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
}

/// Where `installer/` goes for transaction `id`: a sibling of the home.
pub fn aside_dir(home: &Home, id: &str) -> Result<PathBuf, JournalError> {
    if !plain_id(id) {
        return Err(misuse(format!("`{id}` is not a transaction id")));
    }
    let root = home.root();
    let name = root_name(home).ok_or_else(|| misuse("the home has no directory name"))?;
    let parent = root
        .parent()
        .ok_or_else(|| misuse("the home has no parent directory"))?;
    Ok(parent.join(format!("{name}{INFIX}{id}")))
}

/// The id in a sibling's name, when the name is exactly `<home>-uninstall-<id>`.
fn aside_id(home: &Home, path: &Path) -> Option<String> {
    let name = root_name(home)?;
    let file = path.file_name()?.to_string_lossy().into_owned();
    let id = file.strip_prefix(&format!("{name}{INFIX}"))?;
    let expect = aside_dir(home, id).ok()?;
    (plain_id(id) && expect == path).then(|| id.to_string())
}

/// The commit point of `uninstall --all`: make everything durable, then rename `installer/` to
/// the sibling for this transaction. Returns where it went.
pub fn move_installer_aside(txn: &mut Txn, home: &Home) -> Result<PathBuf, JournalError> {
    let aside = aside_dir(home, txn.id())?;
    let from = home.installer();
    if crate::util::exists_any(&aside) {
        return Err(misuse(format!("{} already exists", aside.display())));
    }
    txn.barrier()?;
    // Windows will not move a directory with an open file in it: close the journal for the
    // rename and open it again, by path, where it has gone
    txn.release_journal()?;
    fault_after("uninstall-before-aside");
    platform::rename(&from, &aside).map_err(io(&aside))?;
    txn.relocate(aside.join("txn").join(txn.id()));
    if let Some(p) = aside.parent() {
        let _ = platform::sync_dir(p);
    }
    fault_after("uninstall-aside");
    Ok(aside)
}

/// One `<home>-uninstall-<id>` sibling found next to the home.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Aside {
    pub path: PathBuf,
    pub id: String,
    /// Its journal has the `commit` line; without it the process died between the rename and the
    /// commit line (the uninstall had happened).
    pub committed: bool,
    /// Kept on purpose: `--keep-journal` wrote the marker file. Anything else is a remnant.
    pub kept: bool,
}

/// The file `--keep-journal` leaves in the sibling.
pub const KEPT_MARKER: &str = "kept";

/// Mark a sibling as one to keep.
pub fn mark_kept(path: &Path) -> Result<(), JournalError> {
    let p = path.join(KEPT_MARKER);
    fs::write(&p, "kept with --keep-journal\n").map_err(io(&p))
}

/// The siblings of the home that an `uninstall --all` left, oldest first. Read-only.
pub fn leftover_asides(home: &Home) -> Vec<Aside> {
    let root = home.root();
    let Some(parent) = root.parent() else {
        return Vec::new();
    };
    let Ok(rd) = fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut out: Vec<Aside> = rd
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            let id = aside_id(home, &path)?;
            if !e.file_type().is_ok_and(|t| t.is_dir()) {
                return None;
            }
            let j = path.join("txn").join(&id).join("journal.jsonl");
            let committed = read_journal(&id, &j).is_ok_and(|j| {
                j.lines
                    .iter()
                    .any(|l| matches!(l.op, JournalOp::Commit { .. }))
            });
            let kept = path.join(KEPT_MARKER).is_file();
            Some(Aside {
                path,
                id,
                committed,
                kept,
            })
        })
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// Delete a sibling `uninstall --all` made. Refuses any other path.
pub fn discard_aside(home: &Home, path: &Path) -> Result<(), JournalError> {
    if aside_id(home, path).is_none() {
        return Err(misuse(format!(
            "refusing to delete {}: not an uninstall journal of this home",
            path.display()
        )));
    }
    remove_tree(path).map_err(io(path))
}

/// Remove a tree without following links; directories a toolchain made read-only are opened to
/// the owner first.
fn remove_tree(path: &Path) -> std::io::Result<()> {
    open_to_owner(path);
    match platform::remove_dir_all(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        r => r,
    }
}

#[cfg(unix)]
fn open_to_owner(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let Ok(md) = fs::symlink_metadata(dir) else {
        return;
    };
    if !md.is_dir() {
        return;
    }
    let _ = fs::set_permissions(
        dir,
        fs::Permissions::from_mode(md.permissions().mode() | 0o700),
    );
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                open_to_owner(&e.path());
            }
        }
    }
}
#[cfg(not(unix))]
fn open_to_owner(dir: &Path) {
    platform::clear_readonly_tree(dir);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_sibling_name_is_accepted() {
        let t = tempfile::tempdir().unwrap();
        let h = Home::at(t.path().join(".cigscript"));
        let id = "20261112T100311-a1b2c3";
        let a = aside_dir(&h, id).unwrap();
        assert_eq!(a, t.path().join(format!(".cigscript-uninstall-{id}")));
        assert!(aside_id(&h, &a).is_some());
        assert!(aside_dir(&h, "../x").is_err());
        assert!(aside_dir(&h, "20261112T100311-A1B2C3").is_err());
        for bad in [
            t.path().to_path_buf(),
            t.path().join(".cigscript"),
            t.path().join(".cigscript-uninstall-x"),
            t.path().join(format!("other-uninstall-{id}")),
            t.path()
                .join("sub")
                .join(format!(".cigscript-uninstall-{id}")),
        ] {
            assert!(discard_aside(&h, &bad).is_err(), "{}", bad.display());
        }
    }

    #[test]
    fn leftovers_are_found_and_discarded() {
        let t = tempfile::tempdir().unwrap();
        let h = Home::at(t.path().join(".cigscript"));
        let id = "20261112T100311-a1b2c3";
        let a = aside_dir(&h, id).unwrap();
        fs::create_dir_all(a.join("txn").join(id)).unwrap();
        fs::write(
            a.join("txn").join(id).join("journal.jsonl"),
            "{\"seq\":0,\"t\":\"begin\",\"label\":\"uninstall --all\",\"pid\":1,\"format\":1}\n",
        )
        .unwrap();
        let found = leftover_asides(&h);
        assert_eq!(found.len(), 1);
        assert!(!found[0].committed);
        discard_aside(&h, &found[0].path).unwrap();
        assert!(leftover_asides(&h).is_empty());
    }
}
