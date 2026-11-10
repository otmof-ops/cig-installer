// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! The transaction journal (section 5.5): write-ahead lines, atomic flips,
//! rollback, and recovery of interrupted operations.
//!
//! Every change goes through a [`Txn`]. The op line is written and fsynced
//! BEFORE the change it describes, so replaying the journal backwards always
//! leaves the home as it was. A `Txn` that is dropped without `commit` or
//! `rollback` is, by design, indistinguishable from a crash: the next
//! [`recover`] settles it.

use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::{jio, JournalError};
use crate::home::Home;
use crate::lock::InstallerLock;
use crate::platform;
use crate::store::{ObjectId, Store};
use crate::util;

/// Journal line format.
pub const JOURNAL_FORMAT: u32 = 1;

/// A transaction directory is deleted this long after a clean commit.
pub const TXN_RETENTION_DAYS: u64 = 7;

/// Largest `shim-index` the installer will write (5.3).
pub const MAX_INDEX_BYTES: usize = 64 * 1024;

/// What a path looked like before an op touched it (cig's `Before`, reduced
/// to what the installer needs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Before {
    Absent,
    /// A file stood here. `saved` is a copy under the txn directory (a hard
    /// link when possible); absent for a `store` op, where it only means
    /// "the object was already there".
    File {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        saved: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sha256: Option<String>,
    },
    /// A directory stood here; `saved` is where it was moved to (`delete`).
    Dir {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        saved: Option<String>,
    },
    /// A symbolic link stood here, pointing at `target` (`delete`): it is unlinked, never
    /// followed, and a rollback makes the same link again.
    Link {
        target: String,
    },
}

/// One journal record. The vocabulary is section 5.5's:
/// `begin mkdir store link move write delete block flip commit rollback-done`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum JournalOp {
    Begin {
        label: String,
        pid: u32,
        format: u32,
    },
    Mkdir {
        path: PathBuf,
        before: Before,
    },
    Store {
        object: String,
        exec: bool,
        before: Before,
    },
    Link {
        from: PathBuf,
        to: PathBuf,
        before: Before,
    },
    Move {
        from: PathBuf,
        to: PathBuf,
        before: Before,
    },
    Write {
        path: PathBuf,
        before: Before,
        after_sha256: String,
    },
    Delete {
        path: PathBuf,
        before: Before,
    },
    Block {
        path: PathBuf,
        before: Before,
        marker: String,
    },
    Flip {
        index_sha256: String,
        previous_sha256: String,
        /// Copy of the previous `shim-index`, so a rollback after the rename
        /// can put it back (an extension of the 5.5 line).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        saved: Option<String>,
    },
    Commit {
        /// Set when `recover` finished the commit of a rolled-forward txn.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        recovered: bool,
    },
    #[serde(rename = "rollback-done")]
    RollbackDone,
}

impl JournalOp {
    fn is_control(&self) -> bool {
        matches!(
            self,
            JournalOp::Begin { .. }
                | JournalOp::Flip { .. }
                | JournalOp::Commit { .. }
                | JournalOp::RollbackDone
        )
    }

    fn name(&self) -> &'static str {
        match self {
            JournalOp::Begin { .. } => "begin",
            JournalOp::Mkdir { .. } => "mkdir",
            JournalOp::Store { .. } => "store",
            JournalOp::Link { .. } => "link",
            JournalOp::Move { .. } => "move",
            JournalOp::Write { .. } => "write",
            JournalOp::Delete { .. } => "delete",
            JournalOp::Block { .. } => "block",
            JournalOp::Flip { .. } => "flip",
            JournalOp::Commit { .. } => "commit",
            JournalOp::RollbackDone => "rollback-done",
        }
    }
}

/// A journal line: sequence number plus the record, flat in JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Line {
    pub seq: u64,
    #[serde(flatten)]
    pub op: JournalOp,
}

/// The switch of `shim-index`: the full new text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlipIntent {
    pub index: String,
}

/// One undo that did not work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RollbackFailure {
    pub seq: u64,
    pub what: String,
    pub error: String,
}

/// The outcome of a rollback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RollbackReport {
    pub txn: String,
    pub undone: usize,
    /// Non-empty means `E1833`: `rollback-done` was NOT written and the next
    /// `recover` retries.
    pub failed: Vec<RollbackFailure>,
    /// Things left in place on purpose (a directory that is no longer empty).
    pub notes: Vec<String>,
}

impl RollbackReport {
    pub fn is_complete(&self) -> bool {
        self.failed.is_empty()
    }
}

/// The seam for D3: the installer's own journal today, `cig-journal` later.
pub trait Journal {
    /// Log an op (write-ahead). Control records are refused.
    fn record(&mut self, op: JournalOp) -> Result<(), JournalError>;
    /// The atomic switch of `shim-index`.
    fn flip(&mut self, intent: FlipIntent) -> Result<(), JournalError>;
    fn commit(self) -> Result<(), JournalError>;
    fn rollback(self) -> Result<RollbackReport, JournalError>;
}

// ---------------------------------------------------------------- faults

#[cfg(feature = "fault-injection")]
#[derive(Debug)]
struct Fault {
    phase: String,
    n: u32,
    seen: u32,
}

#[cfg(feature = "fault-injection")]
impl Fault {
    fn from_env() -> Option<Fault> {
        let v = std::env::var("CIGI_FAULT").ok()?;
        let (phase, n) = v.split_once(':')?;
        Some(Fault {
            phase: phase.to_string(),
            n: n.parse().ok()?,
            seen: 0,
        })
    }
}

/// A named point outside any transaction where a test build may stop the process
/// (`CIGI_FAULT=<point>:1`): the moments a journal cannot see, such as after a commit and before
/// the cleanup that follows it. Nothing at all without fault injection.
#[allow(unused_variables)]
pub fn fault_after(point: &str) {
    #[cfg(feature = "fault-injection")]
    if let Some(f) = Fault::from_env() {
        if f.phase == point && f.n <= 1 {
            std::process::abort();
        }
    }
}

/// Is the named fault planted (`CIGI_FAULT=<point>:1`)? For a check that a test build tampers with
/// on purpose, where `fault_after` would stop the process instead. Always false without fault
/// injection.
#[allow(unused_variables)]
pub fn fault_planted(point: &str) -> bool {
    #[cfg(feature = "fault-injection")]
    if let Some(f) = Fault::from_env() {
        return f.phase == point && f.n <= 1;
    }
    false
}

// ------------------------------------------------------------------- Txn

/// An open transaction. See the module docs.
#[derive(Debug)]
pub struct Txn {
    home: Home,
    id: String,
    dir: PathBuf,
    /// The journal, open for append. Closed by `release_journal` for the moment a rename moves
    /// the directory it lives in (Windows will not move a directory with an open file in it),
    /// and opened again by path at the next line.
    file: Option<File>,
    seq: u64,
    flipped: bool,
    saved_n: u32,
    unsynced: bool,
    /// Objects published without fsync, flushed at the next barrier.
    pending: Vec<PathBuf>,
    #[cfg(feature = "fault-injection")]
    fault: Option<Fault>,
    #[cfg(any(test, feature = "fault-injection"))]
    stop_before_rename: bool,
}

impl Txn {
    /// Open a transaction. The lock is the proof that no other installer runs.
    pub fn begin(home: &Home, label: &str, lock: &InstallerLock) -> Result<Txn, JournalError> {
        if !lock.belongs_to(home) {
            return Err(JournalError::Misuse("lock belongs to another home".into()));
        }
        let root = home.txn_root();
        fs::create_dir_all(&root).map_err(jio(&root))?;
        let (id, dir) = loop {
            let id = util::new_txn_id();
            let dir = root.join(&id);
            match fs::create_dir(&dir) {
                Ok(()) => break (id, dir),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(jio(&dir)(e)),
            }
        };
        let saved = dir.join("saved");
        fs::create_dir(&saved).map_err(jio(&saved))?;
        let jpath = dir.join("journal.jsonl");
        let file = OpenOptions::new()
            .create_new(true)
            .append(true)
            .open(&jpath)
            .map_err(jio(&jpath))?;
        let mut txn = Txn {
            home: home.clone(),
            id,
            dir,
            file: Some(file),
            seq: 0,
            flipped: false,
            saved_n: 0,
            unsynced: false,
            pending: Vec::new(),
            #[cfg(feature = "fault-injection")]
            fault: Fault::from_env(),
            #[cfg(any(test, feature = "fault-injection"))]
            stop_before_rename: false,
        };
        txn.log(
            &JournalOp::Begin {
                label: label.to_string(),
                pid: std::process::id(),
                format: JOURNAL_FORMAT,
            },
            true,
        )?;
        let _ = platform::sync_dir(&root);
        let _ = platform::sync_dir(&txn.dir);
        Ok(txn)
    }

    /// `YYYYMMDDTHHMMSS-<6 hex>`.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// `installer/txn/<id>`.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn home(&self) -> &Home {
        &self.home
    }

    /// Test hook: the next `flip` writes its line and stops before the
    /// rename, as a crash would.
    #[cfg(any(test, feature = "fault-injection"))]
    pub fn stop_before_rename(&mut self) {
        self.stop_before_rename = true;
    }

    fn journal_path(&self) -> PathBuf {
        self.dir.join("journal.jsonl")
    }

    #[allow(unused_variables)]
    fn fault_point(&mut self, phase: &str) {
        #[cfg(feature = "fault-injection")]
        if let Some(f) = self.fault.as_mut() {
            if f.phase == phase {
                f.seen += 1;
                if f.seen >= f.n {
                    std::process::abort();
                }
            }
        }
    }

    fn log(&mut self, op: &JournalOp, sync: bool) -> Result<(), JournalError> {
        let line = Line {
            seq: self.seq,
            op: op.clone(),
        };
        let mut text = serde_json::to_string(&line)
            .map_err(|e| JournalError::Misuse(format!("cannot encode journal line: {e}")))?;
        text.push('\n');
        let jp = self.journal_path();
        self.journal_file()?
            .write_all(text.as_bytes())
            .map_err(jio(&jp))?;
        if sync {
            self.journal_file()?.sync_data().map_err(jio(&jp))?;
            self.unsynced = false;
        } else {
            self.unsynced = true;
        }
        self.seq += 1;
        self.fault_point(op.name());
        Ok(())
    }

    /// Make everything so far durable: published store objects, their
    /// directories, and the journal. Called before every step that makes the
    /// work visible (`move`, `flip`, `commit`); callers may also call it.
    pub fn barrier(&mut self) -> Result<(), JournalError> {
        self.sync_journal()
    }

    fn sync_journal(&mut self) -> Result<(), JournalError> {
        if !self.pending.is_empty() {
            let pending = std::mem::take(&mut self.pending);
            sync_files_parallel(&pending)?;
            let shards: std::collections::BTreeSet<&Path> =
                pending.iter().filter_map(|p| p.parent()).collect();
            for s in shards {
                let _ = platform::sync_dir(s);
            }
        }
        if self.unsynced {
            let jp = self.journal_path();
            self.journal_file()?.sync_data().map_err(jio(&jp))?;
            self.unsynced = false;
        }
        Ok(())
    }

    /// The journal file, opened again by path when `release_journal` closed it.
    fn journal_file(&mut self) -> Result<&mut File, JournalError> {
        if self.file.is_none() {
            let jp = self.journal_path();
            let f = OpenOptions::new()
                .append(true)
                .open(&jp)
                .map_err(jio(&jp))?;
            self.file = Some(f);
        }
        self.file
            .as_mut()
            .ok_or_else(|| JournalError::Misuse("the journal is not open".into()))
    }

    /// Make the journal durable and close it, so the directory it is in can be renamed on
    /// Windows. The next line opens it again by path.
    pub(crate) fn release_journal(&mut self) -> Result<(), JournalError> {
        self.sync_journal()?;
        self.file = None;
        Ok(())
    }

    /// The transaction directory moved (the uninstall's aside rename): later lines go to `dir`.
    pub(crate) fn relocate(&mut self, dir: PathBuf) {
        self.dir = dir;
    }

    fn next_saved(&mut self) -> (String, PathBuf) {
        self.saved_n += 1;
        let rel = format!("saved/{:04}", self.saved_n);
        let abs = self.dir.join(&rel);
        (rel, abs)
    }

    /// Record what `path` is now, saving a copy when it is a file.
    fn snapshot(&mut self, path: &Path, dirs_ok: bool) -> Result<Before, JournalError> {
        match fs::symlink_metadata(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Before::Absent),
            Err(e) => Err(jio(path)(e)),
            Ok(md) if md.is_dir() => {
                if dirs_ok {
                    Ok(Before::Dir { saved: None })
                } else {
                    Err(JournalError::Misuse(format!(
                        "{} is a directory",
                        path.display()
                    )))
                }
            }
            Ok(md) if platform::is_link_md(&md, path) => Err(JournalError::Refused {
                path: path.to_path_buf(),
                why: "is a symbolic link, and the installer does not write through one".into(),
            }),
            Ok(_) => {
                let (sha, _) = util::sha256_file(path).map_err(jio(path))?;
                let (rel, abs) = self.next_saved();
                util::link_or_copy(path, &abs).map_err(jio(&abs))?;
                Ok(Before::File {
                    saved: Some(rel),
                    sha256: Some(sha),
                })
            }
        }
    }

    // -------------------------------------------------- the actions

    /// Create one directory (parent must exist). Existing: no-op, no record.
    pub fn mkdir(&mut self, path: &Path) -> Result<(), JournalError> {
        if path.is_dir() {
            return Ok(());
        }
        self.log(
            &JournalOp::Mkdir {
                path: path.to_path_buf(),
                before: Before::Absent,
            },
            true,
        )?;
        match fs::create_dir(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && path.is_dir() => {
                return Ok(())
            }
            Err(e) => return Err(jio(path)(e)),
        }
        // Never the umask's mode (group-write under 002) inside the home.
        if let Some(mode) = self.dir_mode_for(path) {
            platform::set_mode(path, mode).map_err(jio(path))?;
        }
        Ok(())
    }

    /// The mode of a directory this transaction creates: 0700 under
    /// `installer/` and `store/`, 0755 elsewhere inside the home, and the
    /// platform default outside it (a user's `~/.config/fish` is theirs).
    fn dir_mode_for(&self, path: &Path) -> Option<u32> {
        if platform::starts_with(path, &self.home.installer())
            || platform::starts_with(path, &self.home.store())
        {
            Some(0o700)
        } else if platform::starts_with(path, &self.home.root()) {
            Some(0o755)
        } else {
            None
        }
    }

    /// `mkdir -p`, recording each directory it creates, outermost first.
    pub fn mkdir_all(&mut self, path: &Path) -> Result<(), JournalError> {
        let mut missing: Vec<&Path> = Vec::new();
        for a in path.ancestors() {
            if a.as_os_str().is_empty() || a.is_dir() {
                break;
            }
            missing.push(a);
        }
        for a in missing.into_iter().rev() {
            self.mkdir(a)?;
        }
        Ok(())
    }

    /// Put a staged file into the store and record it. Idempotent: an object
    /// that is already there is recorded as such and left alone on rollback.
    pub fn store_file(
        &mut self,
        store: &Store<'_>,
        source: &Path,
        exec: bool,
    ) -> Result<ObjectId, JournalError> {
        let staged = store.stage_file_batched(source, exec)?;
        let existed = store.verify(staged.id()).is_ok();
        let before = if existed {
            Before::File {
                saved: None,
                sha256: None,
            }
        } else {
            Before::Absent
        };
        self.log(
            &JournalOp::Store {
                object: staged.id().sha256.clone(),
                exec,
                before,
            },
            false, // an object without its line is garbage for `gc`, never damage
        )?;
        let id = store.publish(staged)?;
        if !existed && platform::BATCHED_FSYNC {
            self.pending.push(store.object_path(&id));
        }
        Ok(id)
    }

    /// Hard link `from` to `to` (copy when linking is impossible). An
    /// existing `to` that is not already the same file is replaced atomically
    /// and its old content saved for rollback.
    pub fn link(&mut self, from: &Path, to: &Path) -> Result<(), JournalError> {
        if platform::same_file(from, to) {
            return Ok(());
        }
        let before = self.snapshot(to, false)?;
        let in_partial = to.ancestors().any(|a| {
            a.file_name()
                .map(|n| n.to_string_lossy().contains(".partial-"))
                .unwrap_or(false)
        });
        let sync = !(matches!(before, Before::Absent) && in_partial);
        self.log(
            &JournalOp::Link {
                from: from.to_path_buf(),
                to: to.to_path_buf(),
                before: before.clone(),
            },
            sync,
        )?;
        if matches!(before, Before::Absent) {
            util::link_or_copy(from, to).map_err(jio(to))
        } else {
            let tmp = util::temp_sibling(to);
            util::link_or_copy(from, &tmp).map_err(jio(&tmp))?;
            platform::rename(&tmp, to).map_err(|e| {
                let _ = platform::remove_file(&tmp);
                jio(to)(e)
            })
        }
    }

    /// Rename `from` to `to`; `to` must not exist. The directory flip of a
    /// toolchain (4.2).
    pub fn move_path(&mut self, from: &Path, to: &Path) -> Result<(), JournalError> {
        if util::exists_any(to) {
            return Err(JournalError::Misuse(format!(
                "{} already exists",
                to.display()
            )));
        }
        self.sync_journal()?;
        self.log(
            &JournalOp::Move {
                from: from.to_path_buf(),
                to: to.to_path_buf(),
                before: Before::Absent,
            },
            true,
        )?;
        platform::rename(from, to).map_err(jio(to))?;
        if let Some(p) = to.parent() {
            let _ = platform::sync_dir(p);
        }
        Ok(())
    }

    /// Replace a file's content atomically (temp, fsync, rename), saving the
    /// old one. Identical content: nothing is written or recorded.
    pub fn write(
        &mut self,
        path: &Path,
        bytes: &[u8],
        mode: Option<u32>,
    ) -> Result<(), JournalError> {
        self.write_op(path, bytes, mode, None)
    }

    /// As [`Txn::write`], logged as a `block` op (a marked block inside a
    /// user's profile file).
    pub fn write_block(
        &mut self,
        path: &Path,
        marker: &str,
        bytes: &[u8],
        mode: Option<u32>,
    ) -> Result<(), JournalError> {
        self.write_op(path, bytes, mode, Some(marker))
    }

    fn write_op(
        &mut self,
        path: &Path,
        bytes: &[u8],
        mode: Option<u32>,
        marker: Option<&str>,
    ) -> Result<(), JournalError> {
        let after = util::sha256_bytes(bytes);
        if let Ok((cur, _)) = util::sha256_file(path) {
            if cur == after {
                return Ok(());
            }
        }
        let before = self.snapshot(path, false)?;
        let op = match marker {
            None => JournalOp::Write {
                path: path.to_path_buf(),
                before,
                after_sha256: after,
            },
            Some(m) => JournalOp::Block {
                path: path.to_path_buf(),
                before,
                marker: m.to_string(),
            },
        };
        self.log(&op, true)?;
        util::write_atomic(path, bytes, mode).map_err(jio(path))
    }

    /// Delete a file or directory tree by moving it into the txn directory,
    /// so rollback can put it back exactly.
    pub fn delete(&mut self, path: &Path) -> Result<(), JournalError> {
        let root = self.home.root();
        self.delete_within(path, &root)
    }

    /// As [`Txn::delete`] for a file in the user's own home (a profile file the installer made).
    pub fn delete_user_file(&mut self, path: &Path) -> Result<(), JournalError> {
        let base = self.home.user_home();
        self.delete_within(path, &base)
    }

    /// A path reaches a remove only when it lies inside `base` and has no `..` component: names
    /// read from files on disk (`state.json`) end up in these paths, and a name is not trusted
    /// to stay in its place. Refused before anything is looked at or changed.
    fn delete_within(&mut self, path: &Path, base: &Path) -> Result<(), JournalError> {
        let plain = path
            .components()
            .all(|c| !matches!(c, std::path::Component::ParentDir));
        if !plain || !platform::starts_with(path, base) || platform::same_path(path, base) {
            return Err(JournalError::Refused {
                path: path.to_path_buf(),
                why: format!("is not a path inside {}", base.display()),
            });
        }
        // no directory between `base` and the path may be a link: a remove goes through it
        for dir in path.ancestors().skip(1) {
            if platform::same_path(dir, base) || !platform::starts_with(dir, base) {
                break;
            }
            if platform::is_link(dir) {
                return Err(JournalError::Refused {
                    path: dir.to_path_buf(),
                    why: "is a symbolic link where a directory of the installer's should be".into(),
                });
            }
        }
        let md = match fs::symlink_metadata(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(jio(path)(e)),
            Ok(md) => md,
        };
        if platform::is_link_md(&md, path) {
            let target = fs::read_link(path).map_err(jio(path))?;
            self.log(
                &JournalOp::Delete {
                    path: path.to_path_buf(),
                    before: Before::Link {
                        target: target.to_string_lossy().into_owned(),
                    },
                },
                true,
            )?;
            // `remove_file` on a link removes the link itself
            platform::remove_file(path).map_err(jio(path))?;
        } else if md.is_dir() {
            let (rel, abs) = self.next_saved();
            self.log(
                &JournalOp::Delete {
                    path: path.to_path_buf(),
                    before: Before::Dir { saved: Some(rel) },
                },
                true,
            )?;
            platform::rename(path, &abs).map_err(jio(path))?;
        } else {
            let before = self.snapshot(path, false)?;
            self.log(
                &JournalOp::Delete {
                    path: path.to_path_buf(),
                    before,
                },
                true,
            )?;
            platform::remove_file(path).map_err(jio(path))?;
        }
        if let Some(p) = path.parent() {
            let _ = platform::sync_dir(p);
        }
        Ok(())
    }

    /// Roll back what this transaction did. Same as [`Journal::rollback`].
    fn do_rollback(mut self) -> Result<RollbackReport, JournalError> {
        self.sync_journal()?;
        let jr = read_journal(&self.id, &self.journal_path())?;
        let (undone, failed, notes) = undo_all(&self.home, &self.dir, &jr.lines);
        let report = RollbackReport {
            txn: self.id.clone(),
            undone,
            failed,
            notes,
        };
        if report.is_complete() {
            self.log(&JournalOp::RollbackDone, true)?;
        }
        Ok(report)
    }
}

impl Journal for Txn {
    fn record(&mut self, op: JournalOp) -> Result<(), JournalError> {
        if op.is_control() {
            return Err(JournalError::Misuse(format!(
                "`{}` is not a recordable op",
                op.name()
            )));
        }
        self.log(&op, true)
    }

    /// Write `shim-index.new`, log the flip line, then rename. The flip line
    /// is durable BEFORE the rename (5.5). One flip per transaction.
    fn flip(&mut self, intent: FlipIntent) -> Result<(), JournalError> {
        if self.flipped {
            return Err(JournalError::Misuse("a transaction flips once".into()));
        }
        if intent.index.len() > MAX_INDEX_BYTES {
            return Err(JournalError::Misuse(format!(
                "shim-index would be {} bytes; the limit is {MAX_INDEX_BYTES}",
                intent.index.len()
            )));
        }
        self.sync_journal()?;
        let idx = self.home.shim_index_path();
        let new_path = idx.with_file_name("shim-index.new");
        let index_sha = util::sha256_bytes(intent.index.as_bytes());
        let (previous_sha, saved) = match util::sha256_file(&idx) {
            Ok((h, _)) => {
                let (rel, abs) = self.next_saved();
                util::link_or_copy(&idx, &abs).map_err(jio(&abs))?;
                (h, Some(rel))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => ("absent".to_string(), None),
            Err(e) => return Err(jio(&idx)(e)),
        };
        if let Some(p) = idx.parent() {
            fs::create_dir_all(p).map_err(jio(p))?;
        }
        {
            let mut f = File::create(&new_path).map_err(jio(&new_path))?;
            f.write_all(intent.index.as_bytes())
                .map_err(jio(&new_path))?;
            f.sync_all().map_err(jio(&new_path))?;
        }
        self.log(
            &JournalOp::Flip {
                index_sha256: index_sha.clone(),
                previous_sha256: previous_sha.clone(),
                saved,
            },
            true,
        )?;
        self.flipped = true;
        #[cfg(any(test, feature = "fault-injection"))]
        if self.stop_before_rename {
            return Ok(());
        }
        if index_sha == previous_sha {
            let _ = platform::remove_file(&new_path);
        } else {
            platform::rename(&new_path, &idx).map_err(jio(&idx))?;
            if let Some(p) = idx.parent() {
                let _ = platform::sync_dir(p);
            }
        }
        self.fault_point("flipped");
        Ok(())
    }

    fn commit(mut self) -> Result<(), JournalError> {
        self.sync_journal()?;
        self.log(&JournalOp::Commit { recovered: false }, true)
    }

    fn rollback(self) -> Result<RollbackReport, JournalError> {
        self.do_rollback()
    }
}

/// fsync many files. Concurrent fsyncs share filesystem journal commits, which
/// is several times faster than one after another (measured, see the tests).
fn sync_files_parallel(paths: &[PathBuf]) -> Result<(), JournalError> {
    const THREADS: usize = 8;
    if paths.len() < 16 {
        for p in paths {
            platform::sync_file(p).map_err(jio(p))?;
        }
        return Ok(());
    }
    let chunk = paths.len().div_ceil(THREADS);
    let results: Vec<Result<(), JournalError>> = std::thread::scope(|s| {
        let handles: Vec<_> = paths
            .chunks(chunk)
            .map(|c| {
                s.spawn(move || -> Result<(), JournalError> {
                    for p in c {
                        platform::sync_file(p).map_err(jio(p))?;
                    }
                    Ok(())
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| {
                h.join()
                    .unwrap_or_else(|_| Err(JournalError::Misuse("fsync worker panicked".into())))
            })
            .collect()
    });
    results.into_iter().collect()
}

// -------------------------------------------------------- reading journals

pub(crate) struct JournalFile {
    pub lines: Vec<Line>,
    /// Bytes up to and including the last complete line.
    pub valid_len: u64,
    /// A partial last line was present (a crash mid-write).
    pub torn: bool,
}

/// Parse a journal. A torn tail is dropped (its action never began: the line
/// is written before the action). Anything else unreadable is `E1891`.
pub(crate) fn read_journal(txn: &str, path: &Path) -> Result<JournalFile, JournalError> {
    let corrupt = |why: String| JournalError::Corrupt {
        txn: txn.to_string(),
        why,
    };
    let bytes = fs::read(path).map_err(jio(path))?;
    let text = String::from_utf8(bytes).map_err(|_| corrupt("not UTF-8".into()))?;
    let mut lines = Vec::new();
    let mut valid_len = 0u64;
    let mut rest = text.as_str();
    let mut torn = false;
    while !rest.is_empty() {
        match rest.find('\n') {
            None => {
                torn = true;
                break;
            }
            Some(i) => {
                let raw = &rest[..i];
                let line: Line = serde_json::from_str(raw)
                    .map_err(|e| corrupt(format!("line {}: {e}", lines.len())))?;
                if line.seq != lines.len() as u64 {
                    return Err(corrupt(format!(
                        "line {} has seq {}",
                        lines.len(),
                        line.seq
                    )));
                }
                lines.push(line);
                valid_len += (i + 1) as u64;
                rest = &rest[i + 1..];
            }
        }
    }
    match lines.first() {
        Some(Line {
            op: JournalOp::Begin { format, .. },
            ..
        }) => {
            if *format > JOURNAL_FORMAT {
                return Err(JournalError::TooNew {
                    txn: txn.to_string(),
                    found: *format,
                });
            }
        }
        Some(_) => return Err(corrupt("first line is not `begin`".into())),
        None => {}
    }
    Ok(JournalFile {
        lines,
        valid_len,
        torn,
    })
}

fn label_of(lines: &[Line]) -> String {
    match lines.first() {
        Some(Line {
            op: JournalOp::Begin { label, .. },
            ..
        }) => label.clone(),
        _ => String::new(),
    }
}

// ------------------------------------------------------------------- undo

fn restore_saved(txn_dir: &Path, saved: &str, sha: Option<&str>, to: &Path) -> Result<(), String> {
    let s = txn_dir.join(saved);
    if !s.is_file() {
        return Err(format!("saved copy {saved} is missing"));
    }
    if let Some(want) = sha {
        let (got, _) = util::sha256_file(&s).map_err(|e| format!("{saved}: {e}"))?;
        if got != want {
            return Err(format!("saved copy {saved} changed (hash {got})"));
        }
    }
    if platform::same_file(&s, to) {
        return Ok(()); // already that very file
    }
    let tmp = util::temp_sibling(to);
    util::link_or_copy(&s, &tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    let res = platform::rename(&tmp, to).map_err(|e| format!("{}: {e}", to.display()));
    // rename(2) leaves the source in place when both are links of one file
    let _ = platform::remove_file(&tmp);
    res
}

/// Every `(sha256, exec)` the installed toolchains' receipts name, built once
/// per undo pass (a rollback of n files must not read every receipt n times).
#[derive(Debug, Default)]
struct InUse {
    objects: std::collections::HashSet<(String, bool)>,
    /// Some receipt exists but cannot be read: nothing can be ruled out.
    unreadable: bool,
}

impl InUse {
    fn scan(home: &Home) -> InUse {
        let mut out = InUse::default();
        let Ok(tools) = fs::read_dir(home.toolchains()) else {
            return out;
        };
        for tool in tools.flatten() {
            let Ok(versions) = fs::read_dir(tool.path()) else {
                continue;
            };
            for v in versions.flatten() {
                // `.<ver>.partial-<txn>` directories are work in flight, not users.
                if v.file_name().to_string_lossy().starts_with('.') {
                    continue;
                }
                let r = v.path().join(crate::toolchain::RECEIPT_NAME);
                let Ok(text) = fs::read_to_string(&r) else {
                    continue;
                };
                let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) else {
                    out.unreadable = true;
                    continue;
                };
                if let Some(files) = val.get("files").and_then(|f| f.as_array()) {
                    for f in files {
                        if let (Some(s), Some(e)) = (
                            f.get("sha256").and_then(|s| s.as_str()),
                            f.get("exec").and_then(|e| e.as_bool()),
                        ) {
                            out.objects.insert((s.to_string(), e));
                        }
                    }
                }
            }
        }
        out
    }
}

/// A store object that something still uses must stay when its `store` op is
/// undone: another receipt names it, or another hard link exists.
fn object_in_use(home: &Home, cache: &mut Option<InUse>, id: &ObjectId) -> bool {
    let store = Store::new(home);
    if platform::nlink(&store.object_path(id))
        .map(|n| n > 1)
        .unwrap_or(false)
    {
        return true;
    }
    let set = cache.get_or_insert_with(|| InUse::scan(home));
    set.unreadable || set.objects.contains(&(id.sha256.clone(), id.exec))
}

/// Is this a toolchain work-in-progress directory the installer owns outright?
fn is_partial_dir(home: &Home, path: &Path) -> bool {
    platform::starts_with(path, &home.toolchains())
        && path
            .file_name()
            .map(|n| {
                let n = n.to_string_lossy();
                n.starts_with('.') && n.contains(".partial-")
            })
            .unwrap_or(false)
}

/// Undo a `mkdir`: remove the directory only when it is empty. Something put
/// there since (a shell's own config, say) is the user's and stays.
fn undo_mkdir(home: &Home, path: &Path, notes: &mut Vec<String>) -> Result<(), String> {
    if is_partial_dir(home, path) {
        return platform::remove_any(path).map_err(|e| format!("{}: {e}", path.display()));
    }
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("{}: {e}", path.display())),
        Ok(md) if !md.is_dir() => {
            notes.push(format!(
                "{} is no longer a directory; left in place",
                path.display()
            ));
            Ok(())
        }
        Ok(_) => match fs::remove_dir(path) {
            Ok(()) => Ok(()),
            Err(e) => {
                let occupied = fs::read_dir(path)
                    .map(|mut r| r.next().is_some())
                    .unwrap_or(false);
                if occupied {
                    notes.push(format!(
                        "{} was created by an interrupted install but now holds other files; left in place",
                        path.display()
                    ));
                    Ok(())
                } else {
                    Err(format!("{}: {e}", path.display()))
                }
            }
        },
    }
}

/// Undo something that created or replaced a file: remove a file or link,
/// never a directory tree.
fn undo_file(path: &Path, notes: &mut Vec<String>) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("{}: {e}", path.display())),
        Ok(md) if md.is_dir() => {
            notes.push(format!(
                "{} is a directory now; left in place",
                path.display()
            ));
            Ok(())
        }
        Ok(_) => platform::remove_file(path).map_err(|e| format!("{}: {e}", path.display())),
    }
}

/// Undo a `move`: put what was moved back where it came from, whole. A toolchain flip
/// (a `.partial-` directory moved into place) is the one move the installer undoes by removing
/// what arrived: that tree is its own, and the next install builds it again. Idempotent: with
/// nothing at `to` the move has not happened or is already undone.
fn undo_move(home: &Home, from: &Path, to: &Path, notes: &mut Vec<String>) -> Result<(), String> {
    if !util::exists_any(to) {
        return Ok(());
    }
    if platform::starts_with(to, &home.toolchains()) && is_partial_dir(home, from) {
        return platform::remove_any(to).map_err(|e| format!("{}: {e}", to.display()));
    }
    if util::exists_any(from) {
        notes.push(format!(
            "{} was moved to {}, and something is at {} now; both left in place",
            from.display(),
            to.display(),
            from.display()
        ));
        return Ok(());
    }
    platform::rename(to, from).map_err(|e| format!("{}: {e}", from.display()))
}

fn undo_one(
    home: &Home,
    txn_dir: &Path,
    op: &JournalOp,
    cache: &mut Option<InUse>,
    notes: &mut Vec<String>,
) -> Result<(), String> {
    let rm = |p: &Path| platform::remove_any(p).map_err(|e| format!("{}: {e}", p.display()));
    match op {
        JournalOp::Begin { .. } | JournalOp::Commit { .. } | JournalOp::RollbackDone => Ok(()),
        JournalOp::Mkdir { path, before } => match before {
            Before::Absent => undo_mkdir(home, path, notes),
            _ => Ok(()),
        },
        JournalOp::Store {
            object,
            exec,
            before,
        } => {
            if !matches!(before, Before::Absent) {
                return Ok(());
            }
            let id = ObjectId::new(object.clone(), *exec);
            if object_in_use(home, cache, &id) {
                return Ok(());
            }
            Store::new(home).remove(&id).map_err(|e| e.to_string())
        }
        JournalOp::Link { to, before, .. }
        | JournalOp::Write {
            path: to, before, ..
        }
        | JournalOp::Block {
            path: to, before, ..
        } => match before {
            Before::Absent => undo_file(to, notes),
            Before::File {
                saved: Some(s),
                sha256,
            } => restore_saved(txn_dir, s, sha256.as_deref(), to),
            _ => Ok(()),
        },
        JournalOp::Move { from, to, before } => {
            // the receipts on disk change here: the in-use set is stale
            *cache = None;
            match before {
                Before::Absent => undo_move(home, from, to, notes),
                _ => Ok(()),
            }
        }
        JournalOp::Delete { path, before } => {
            *cache = None;
            undo_delete(txn_dir, path, before)
        }
        JournalOp::Flip {
            index_sha256,
            previous_sha256,
            saved,
        } => {
            let idx = home.shim_index_path();
            let _ = platform::remove_file(&idx.with_file_name("shim-index.new"));
            let cur = util::sha256_file(&idx).ok().map(|(h, _)| h);
            if cur.as_deref() == Some(index_sha256.as_str()) && index_sha256 != previous_sha256 {
                match saved {
                    Some(s) => restore_saved(txn_dir, s, Some(previous_sha256), &idx),
                    None => rm(&idx),
                }
            } else {
                Ok(())
            }
        }
    }
}

#[cfg(unix)]
fn make_link(target: &str, at: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, at)
}
#[cfg(not(unix))]
fn make_link(_target: &str, _at: &Path) -> std::io::Result<()> {
    Err(std::io::Error::other(
        "a symbolic link cannot be made again on this platform",
    ))
}

/// A directory tree with no file in it (what `ensure_layout` leaves).
fn only_empty_dirs(p: &Path) -> bool {
    match fs::read_dir(p) {
        Ok(rd) => rd.flatten().all(|e| {
            e.file_type().is_ok_and(|t| t.is_dir() && !t.is_symlink()) && only_empty_dirs(&e.path())
        }),
        Err(_) => false,
    }
}

fn undo_delete(txn_dir: &Path, path: &Path, before: &Before) -> Result<(), String> {
    match before {
        Before::File {
            saved: Some(s),
            sha256,
        } => {
            if util::exists_any(path) {
                Ok(())
            } else {
                restore_saved(txn_dir, s, sha256.as_deref(), path)
            }
        }
        Before::Link { target } => {
            if fs::symlink_metadata(path).is_ok() {
                return Ok(());
            }
            make_link(target, path).map_err(|e| format!("{}: {e}", path.display()))
        }
        Before::Dir { saved: Some(s) } => {
            let abs = txn_dir.join(s);
            // `ensure_layout` runs before `recover` and makes an empty directory where a whole
            // one was moved aside (`uninstall --all`): an empty one is not what was deleted
            if path.is_dir() && abs.is_dir() && only_empty_dirs(path) {
                platform::remove_dir_all(path).map_err(|e| format!("{}: {e}", path.display()))?;
            }
            if util::exists_any(path) {
                Ok(())
            } else if abs.is_dir() {
                platform::rename(&abs, path).map_err(|e| format!("{}: {e}", path.display()))
            } else {
                Err(format!("saved directory {s} is missing"))
            }
        }
        _ => Ok(()),
    }
}

/// Undo every op, newest first. Idempotent: safe to run again after a crash
/// in the middle of a rollback. Linear in the ops: the receipts are read once.
fn undo_all(
    home: &Home,
    txn_dir: &Path,
    lines: &[Line],
) -> (usize, Vec<RollbackFailure>, Vec<String>) {
    let mut undone = 0;
    let mut failed = Vec::new();
    let mut notes = Vec::new();
    let mut cache: Option<InUse> = None;
    for l in lines.iter().rev() {
        if matches!(
            l.op,
            JournalOp::Begin { .. } | JournalOp::Commit { .. } | JournalOp::RollbackDone
        ) {
            continue;
        }
        match undo_one(home, txn_dir, &l.op, &mut cache, &mut notes) {
            Ok(()) => undone += 1,
            Err(error) => failed.push(RollbackFailure {
                seq: l.seq,
                what: l.op.name().to_string(),
                error,
            }),
        }
    }
    (undone, failed, notes)
}

// ---------------------------------------------------------------- recovery

/// What `recover` did with one interrupted transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoverOutcome {
    /// Undone (no flip, or the flip rename never happened).
    RolledBack { undone: usize },
    /// The flip had happened: the commit was finished.
    RolledForward,
    /// The transaction had already ended (a `commit` or `rollback-done` line)
    /// but its journal is damaged after that line: the damage was cut off.
    /// A note, not a problem.
    Settled { why: String },
    /// Unreadable or inconsistent journal: nothing touched (`E1891`).
    Corrupt { why: String },
    /// Some undo failed: `rollback-done` not written, retried next time (`E1833`).
    Incomplete { failed: Vec<RollbackFailure> },
}

/// One recovered transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recovered {
    pub txn: String,
    pub label: String,
    pub outcome: RecoverOutcome,
    /// Things left in place on purpose.
    pub notes: Vec<String>,
}

impl Recovered {
    /// `E1832` (a note) when recovered, `E1891` or `E1833` when not.
    pub fn code(&self) -> &'static str {
        match self.outcome {
            RecoverOutcome::RolledBack { .. }
            | RecoverOutcome::RolledForward
            | RecoverOutcome::Settled { .. } => "E1832",
            RecoverOutcome::Corrupt { .. } => "E1891",
            RecoverOutcome::Incomplete { .. } => "E1833",
        }
    }

    /// True when the home may not be consistent: the caller must refuse to go on.
    pub fn is_problem(&self) -> bool {
        matches!(
            self.outcome,
            RecoverOutcome::Corrupt { .. } | RecoverOutcome::Incomplete { .. }
        )
    }
}

/// Settle every interrupted transaction under the lock. Called at the start
/// of every mutating command and by `doctor` (which should only report).
///
/// For each open journal: no `flip` line, or a flip whose rename did not
/// happen (`shim-index` still hashes to `previous_sha256`, or the flip changed
/// nothing): roll back. A flip whose rename happened (`shim-index` hashes to
/// `index_sha256`): finish the commit. Anything else: `Corrupt`, untouched.
/// An `Err` is only for I/O trouble reading the txn directory; problems with
/// a journal are `Recovered` items with `is_problem()`.
pub fn recover(home: &Home, lock: &InstallerLock) -> Result<Vec<Recovered>, JournalError> {
    if !lock.belongs_to(home) {
        return Err(JournalError::Misuse("lock belongs to another home".into()));
    }
    let root = home.txn_root();
    let mut ids: Vec<String> = match fs::read_dir(&root) {
        Ok(rd) => rd
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(jio(&root)(e)),
    };
    ids.sort();
    let mut out = Vec::new();
    for id in ids {
        if let Some(r) = recover_one(home, &root.join(&id), &id)? {
            out.push(r);
        }
    }
    if out.iter().all(|r| !r.is_problem()) {
        sweep_debris(home);
    }
    Ok(out)
}

fn recover_one(home: &Home, dir: &Path, id: &str) -> Result<Option<Recovered>, JournalError> {
    let jpath = dir.join("journal.jsonl");
    if !jpath.is_file() {
        // a crash between creating the directory and the first journal line
        let _ = platform::remove_dir_all(dir);
        return Ok(None);
    }
    let mk = |label: String, outcome| {
        Ok(Some(Recovered {
            txn: id.to_string(),
            label,
            outcome,
            notes: Vec::new(),
        }))
    };
    let jr = match read_journal(id, &jpath) {
        Ok(j) => j,
        Err(JournalError::Corrupt { why, .. }) => {
            // Damage after the line that ended the transaction changes nothing
            // about the home: cut it off and carry on.
            if let Some((label, keep)) = settled_prefix(&jpath) {
                let f = OpenOptions::new()
                    .write(true)
                    .open(&jpath)
                    .map_err(jio(&jpath))?;
                f.set_len(keep).map_err(jio(&jpath))?;
                f.sync_all().map_err(jio(&jpath))?;
                return mk(
                    label,
                    RecoverOutcome::Settled {
                        why: format!("the journal was damaged after the transaction ended ({why}); the damage was cut off"),
                    },
                );
            }
            return mk(String::new(), RecoverOutcome::Corrupt { why });
        }
        Err(JournalError::TooNew { found, .. }) => {
            return mk(
                String::new(),
                RecoverOutcome::Corrupt {
                    why: format!("format {found} is newer than this installer"),
                },
            )
        }
        Err(e) => return Err(e),
    };
    let label = label_of(&jr.lines);
    if jr.lines.is_empty() {
        // torn before the first line was complete: nothing ever began
        let _ = platform::remove_dir_all(dir);
        return Ok(None);
    }
    if jr
        .lines
        .iter()
        .any(|l| matches!(l.op, JournalOp::Commit { .. } | JournalOp::RollbackDone))
    {
        return Ok(None);
    }
    let flip = jr.lines.iter().find_map(|l| match &l.op {
        JournalOp::Flip {
            index_sha256,
            previous_sha256,
            ..
        } => Some((index_sha256.clone(), previous_sha256.clone())),
        _ => None,
    });
    let forward = match &flip {
        None => false,
        Some((new, prev)) => {
            let cur = util::sha256_file(&home.shim_index_path())
                .ok()
                .map(|(h, _)| h)
                .unwrap_or_else(|| "absent".to_string());
            if new != prev && &cur == new {
                true
            } else if &cur == prev {
                false
            } else {
                return mk(
                    label,
                    RecoverOutcome::Corrupt {
                        why: format!("shim-index is {cur}, matching neither the flip's new nor previous hash"),
                    },
                );
            }
        }
    };
    if jr.torn {
        let f = OpenOptions::new()
            .write(true)
            .open(&jpath)
            .map_err(jio(&jpath))?;
        f.set_len(jr.valid_len).map_err(jio(&jpath))?;
        f.sync_all().map_err(jio(&jpath))?;
    }
    let mut f = OpenOptions::new()
        .append(true)
        .open(&jpath)
        .map_err(jio(&jpath))?;
    let mut append = |op: JournalOp, seq: u64| -> Result<(), JournalError> {
        let mut text = serde_json::to_string(&Line { seq, op })
            .map_err(|e| JournalError::Misuse(e.to_string()))?;
        text.push('\n');
        f.write_all(text.as_bytes()).map_err(jio(&jpath))?;
        f.sync_data().map_err(jio(&jpath))
    };
    let next = jr.lines.len() as u64;
    if forward {
        let _ = platform::remove_file(&home.shim_index_path().with_file_name("shim-index.new"));
        append(JournalOp::Commit { recovered: true }, next)?;
        return mk(label, RecoverOutcome::RolledForward);
    }
    let (undone, failed, notes) = undo_all(home, dir, &jr.lines);
    let outcome = if failed.is_empty() {
        append(JournalOp::RollbackDone, next)?;
        RecoverOutcome::RolledBack { undone }
    } else {
        RecoverOutcome::Incomplete { failed }
    };
    Ok(Some(Recovered {
        txn: id.to_string(),
        label,
        outcome,
        notes,
    }))
}

/// For a journal that does not parse to the end: the label and the byte
/// length of the valid prefix, when that prefix already holds the line that
/// ended the transaction (`commit` or `rollback-done`).
fn settled_prefix(path: &Path) -> Option<(String, u64)> {
    let text = String::from_utf8_lossy(&fs::read(path).ok()?).into_owned();
    let mut lines: Vec<Line> = Vec::new();
    let mut len = 0u64;
    for raw in text.split_inclusive('\n') {
        let body = raw.strip_suffix('\n')?; // a torn tail has no end line
        let line: Line = serde_json::from_str(body).ok()?;
        if line.seq != lines.len() as u64 {
            return None;
        }
        len += raw.len() as u64;
        let ended = matches!(line.op, JournalOp::Commit { .. } | JournalOp::RollbackDone);
        lines.push(line);
        if ended {
            return Some((label_of(&lines), len));
        }
    }
    None
}

/// With every txn settled and the lock held, nothing may be in flight: drop
/// store stagings, half-built toolchain directories and shim-index debris.
fn sweep_debris(home: &Home) {
    Store::new(home).sweep_tmp();
    let _ = platform::remove_file(&home.shim_index_path().with_file_name("shim-index.new"));
    if let Ok(tools) = fs::read_dir(home.toolchains()) {
        for t in tools.flatten() {
            if let Ok(vs) = fs::read_dir(t.path()) {
                for v in vs.flatten() {
                    let n = v.file_name().to_string_lossy().into_owned();
                    if n.starts_with('.') && n.contains(".partial-") {
                        let _ = platform::remove_any(&v.path());
                    }
                }
            }
        }
    }
}

// ------------------------------------------------------------ housekeeping

/// How a transaction ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxnStatus {
    Committed,
    RolledBack,
    /// No commit or rollback line: interrupted, `recover` will settle it.
    Open,
    Unreadable,
}

/// One entry of `installer/txn/` (for `doctor`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxnInfo {
    pub id: String,
    pub label: String,
    pub status: TxnStatus,
}

/// Every transaction on disk, oldest first. Read-only.
pub fn list_txns(home: &Home) -> Vec<TxnInfo> {
    let root = home.txn_root();
    let mut ids: Vec<String> = fs::read_dir(&root)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().is_dir())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    ids.sort();
    ids.into_iter()
        .map(|id| {
            let p = root.join(&id).join("journal.jsonl");
            match read_journal(&id, &p) {
                Ok(j) => {
                    let status = if j
                        .lines
                        .iter()
                        .any(|l| matches!(l.op, JournalOp::Commit { .. }))
                    {
                        TxnStatus::Committed
                    } else if j
                        .lines
                        .iter()
                        .any(|l| matches!(l.op, JournalOp::RollbackDone))
                    {
                        TxnStatus::RolledBack
                    } else {
                        TxnStatus::Open
                    };
                    TxnInfo {
                        label: label_of(&j.lines),
                        id,
                        status,
                    }
                }
                Err(_) => TxnInfo {
                    id,
                    label: String::new(),
                    status: TxnStatus::Unreadable,
                },
            }
        })
        .collect()
}

/// Delete cleanly committed transactions older than `max_age` (default
/// [`TXN_RETENTION_DAYS`]). Failed, rolled-back and open ones stay for
/// `doctor`. Returns the ids removed.
pub fn prune_txns(
    home: &Home,
    lock: &InstallerLock,
    max_age: std::time::Duration,
) -> Result<Vec<String>, JournalError> {
    if !lock.belongs_to(home) {
        return Err(JournalError::Misuse("lock belongs to another home".into()));
    }
    let mut gone = Vec::new();
    for t in list_txns(home) {
        if t.status != TxnStatus::Committed {
            continue;
        }
        let dir = home.txn_root().join(&t.id);
        let age = fs::metadata(dir.join("journal.jsonl"))
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| m.elapsed().ok());
        if age.map(|a| a >= max_age).unwrap_or(false) {
            platform::remove_dir_all(&dir).map_err(jio(&dir))?;
            gone.push(t.id);
        }
    }
    Ok(gone)
}
