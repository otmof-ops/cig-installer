// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! The move of an existing home's objects from `store/` to `humidor/store/` (ADR 0001 section 3).
//!
//! It is one journaled transaction, `migrate store`: make `humidor/store/`, move `store/objects`
//! under it (one rename, nothing is copied), make `humidor/store/tmp`, and delete the old
//! `store/` by moving it into the transaction's own directory. Every step can be undone, so a
//! crash anywhere is rolled back by the next command's `recover`, and a failure rolls the whole
//! thing back before the error is returned. Hard links from `toolchains/` and `bin/` follow the
//! rename, because a rename does not change what an inode is.

use crate::error::JournalError;
use crate::home::Home;
use crate::journal::{Journal, Txn};
use crate::lock::InstallerLock;
use crate::store::StoreUsage;

/// What a migration moved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreMigration {
    /// The transaction's id.
    pub txn: String,
    /// The objects that were in the old store, which are the objects that are in the new one.
    pub moved: StoreUsage,
}

/// Move the objects of a home made by the old layout under the humidor. `Ok(None)` when there is
/// nothing to move (a new home, or one already moved), so it is safe to run at the start of every
/// command that writes. The caller holds the installer lock and has run `recover`.
pub fn migrate_store(
    home: &Home,
    lock: &InstallerLock,
) -> Result<Option<StoreMigration>, JournalError> {
    if !home.is_legacy_store() {
        return Ok(None);
    }
    let moved = home.object_store().du();
    let mut txn = Txn::begin(home, "migrate store", lock)?;
    let id = txn.id().to_string();
    match migrate_in(&mut txn, home) {
        Ok(()) => {
            txn.commit()?;
            Ok(Some(StoreMigration { txn: id, moved }))
        }
        Err(e) => {
            // the first error is the one that matters; a rollback that also fails is left to
            // `recover`, which retries it
            let _ = txn.rollback();
            Err(e)
        }
    }
}

/// The steps of the migration inside an open transaction (also what the tests undo).
pub fn migrate_in(txn: &mut Txn, home: &Home) -> Result<(), JournalError> {
    let old = home.legacy_store();
    let new = home.shared_store();
    txn.mkdir_all(&new)?;
    txn.move_path(&old.join("objects"), &new.join("objects"))?;
    txn.mkdir(&new.join("tmp"))?;
    txn.delete(&old)?;
    Ok(())
}
