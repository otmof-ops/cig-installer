// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0

//! `cigi-home`: the filesystem half of ciginstaller. Paths and layout, the
//! lock, the content-addressed store, toolchains and receipts, the
//! write-ahead journal with crash recovery, `state.json`, `config`, the
//! `shim-index`, shims, the PATH block and the humidor mirror.
//!
//! Everything that changes the home goes through a [`Txn`]; every
//! operation is idempotent so a replay after a crash does the same thing.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

pub mod config;
pub mod error;
pub mod home;
pub mod humidor;
pub mod index;
pub mod journal;
pub mod lock;
pub mod path;
pub mod platform;
pub mod shims;
pub mod state;
pub mod store;
pub mod toolchain;
mod util;

pub use config::{Config, KeySpec, KEYS};
pub use error::{
    HomeError, HumidorError, JournalError, LockError, PathError, ShimError, StateError, StoreError,
    ToolchainError,
};
pub use home::Home;
pub use humidor::{mirror_humidor, remove_humidor_mirror, HumidorEntry, HumidorMirror};
pub use index::ShimIndex;
pub use journal::{
    list_txns, prune_txns, recover, Before, FlipIntent, Journal, JournalOp, RecoverOutcome,
    Recovered, RollbackFailure, RollbackReport, Txn, TxnInfo, TxnStatus,
};
pub use lock::InstallerLock;
pub use path::{add_path_block, path_block, remove_path_block, PathChange, Shell};
pub use shims::{remove_shims, shim_path, sync_shims};
pub use state::State;
pub use store::{ObjectId, StagedObject, Store, StoreUsage};
pub use toolchain::{
    installed, materialize, read_receipt, verify_toolchain, Damage, Provenance, Receipt,
    ReceiptArtifact, ReceiptFile, ReceiptSource, ReceiptTrust, ShimRef, StagedFile,
};
pub use util::now_rfc3339;

#[cfg(test)]
mod journal_tests;
