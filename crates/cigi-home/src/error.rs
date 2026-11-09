// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0

//! Error types. Every error carries the registered `E18xx` code it maps to.

use std::fmt;
use std::io;
use std::path::PathBuf;

/// `E1836` for a full disk (ENOSPC 28, Windows ERROR_DISK_FULL 112), otherwise the given code.
pub fn io_code(e: &io::Error, otherwise: &'static str) -> &'static str {
    if matches!(e.raw_os_error(), Some(28) | Some(112)) {
        "E1836"
    } else {
        otherwise
    }
}

macro_rules! from_impl {
    ($to:ident :: $variant:ident ( $from:ty )) => {
        impl From<$from> for $to {
            fn from(e: $from) -> Self {
                $to::$variant(e)
            }
        }
    };
}

/// Home discovery, layout and permissions.
#[derive(Debug)]
pub enum HomeError {
    /// Neither `CIGSCRIPT_HOME`, `HOME` nor `USERPROFILE` is set.
    NoHome,
    /// A directory is writable by others or owned by someone else (4.1).
    Unsafe {
        path: PathBuf,
        why: String,
    },
    Io {
        path: PathBuf,
        source: io::Error,
    },
}

impl HomeError {
    pub fn code(&self) -> &'static str {
        match self {
            HomeError::Io { source, .. } => io_code(source, "E1835"),
            _ => "E1835",
        }
    }
}

impl fmt::Display for HomeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HomeError::NoHome => write!(f, "cannot find a home directory (set CIGSCRIPT_HOME)"),
            HomeError::Unsafe { path, why } => {
                write!(f, "{} is unsafe: {why}", path.display())
            }
            HomeError::Io { path, source } => write!(f, "{}: {source}", path.display()),
        }
    }
}

impl std::error::Error for HomeError {}

/// The installer lock.
#[derive(Debug)]
pub enum LockError {
    /// Another ciginstaller holds it past the wait (E1834).
    Held {
        path: PathBuf,
        pid: Option<u32>,
    },
    Io {
        path: PathBuf,
        source: io::Error,
    },
}

impl LockError {
    pub fn code(&self) -> &'static str {
        match self {
            LockError::Held { .. } => "E1834",
            LockError::Io { source, .. } => io_code(source, "E1835"),
        }
    }
}

impl fmt::Display for LockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LockError::Held { path, pid } => match pid {
                Some(p) => write!(f, "another ciginstaller (pid {p}) holds {}", path.display()),
                None => write!(f, "another ciginstaller holds {}", path.display()),
            },
            LockError::Io { path, source } => write!(f, "{}: {source}", path.display()),
        }
    }
}

impl std::error::Error for LockError {}

/// The content-addressed store.
#[derive(Debug)]
pub enum StoreError {
    Io {
        path: PathBuf,
        source: io::Error,
    },
    /// The object is not in the store.
    Missing {
        object: String,
    },
    /// The object's bytes do not hash to its name.
    Mismatch {
        object: String,
        actual: String,
    },
    /// The object's mode is not what its name says.
    Mode {
        object: String,
    },
}

impl StoreError {
    pub fn code(&self) -> &'static str {
        match self {
            StoreError::Io { source, .. } => io_code(source, "E1835"),
            _ => "E1831",
        }
    }
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::Io { path, source } => write!(f, "{}: {source}", path.display()),
            StoreError::Missing { object } => write!(f, "store object {object} is missing"),
            StoreError::Mismatch { object, actual } => {
                write!(f, "store object {object} hashes to {actual}")
            }
            StoreError::Mode { object } => write!(f, "store object {object} has the wrong mode"),
        }
    }
}

impl std::error::Error for StoreError {}

/// The transaction journal.
#[derive(Debug)]
pub enum JournalError {
    Io {
        path: PathBuf,
        source: io::Error,
    },
    /// A journal line is unreadable or out of order (E1891).
    Corrupt {
        txn: String,
        why: String,
    },
    /// A journal from a newer installer (E1830).
    TooNew {
        txn: String,
        found: u32,
    },
    /// A caller broke a rule of the API (E1890): a second flip, an op after
    /// commit, an index over 64 KB.
    Misuse(String),
    Store(StoreError),
}

impl JournalError {
    pub fn code(&self) -> &'static str {
        match self {
            JournalError::Io { source, .. } => io_code(source, "E1835"),
            JournalError::Corrupt { .. } => "E1891",
            JournalError::TooNew { .. } => "E1830",
            JournalError::Misuse(_) => "E1890",
            JournalError::Store(e) => e.code(),
        }
    }
}

impl fmt::Display for JournalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JournalError::Io { path, source } => write!(f, "{}: {source}", path.display()),
            JournalError::Corrupt { txn, why } => write!(f, "journal {txn} is corrupt: {why}"),
            JournalError::TooNew { txn, found } => {
                write!(
                    f,
                    "journal {txn} has format {found}, newer than this installer knows"
                )
            }
            JournalError::Misuse(m) => write!(f, "internal error: {m}"),
            JournalError::Store(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for JournalError {}
from_impl!(JournalError::Store(StoreError));

/// Toolchains and receipts.
#[derive(Debug)]
pub enum ToolchainError {
    Io {
        path: PathBuf,
        source: io::Error,
    },
    /// No receipt (or no toolchain) for this tool and version.
    NoReceipt {
        tool: String,
        version: String,
    },
    /// The receipt is unreadable (E1830).
    BadReceipt {
        path: PathBuf,
        why: String,
    },
    /// A file path in the staged set is not a safe relative path (E1890).
    BadPath(String),
    Store(StoreError),
    Journal(JournalError),
}

impl ToolchainError {
    pub fn code(&self) -> &'static str {
        match self {
            ToolchainError::Io { source, .. } => io_code(source, "E1835"),
            ToolchainError::NoReceipt { .. } => "E1831",
            ToolchainError::BadReceipt { .. } => "E1830",
            ToolchainError::BadPath(_) => "E1890",
            ToolchainError::Store(e) => e.code(),
            ToolchainError::Journal(e) => e.code(),
        }
    }
}

impl fmt::Display for ToolchainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ToolchainError::Io { path, source } => write!(f, "{}: {source}", path.display()),
            ToolchainError::NoReceipt { tool, version } => {
                write!(f, "no receipt for {tool} {version}")
            }
            ToolchainError::BadReceipt { path, why } => {
                write!(f, "receipt {} unreadable: {why}", path.display())
            }
            ToolchainError::BadPath(p) => write!(f, "unsafe path in toolchain: {p}"),
            ToolchainError::Store(e) => e.fmt(f),
            ToolchainError::Journal(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for ToolchainError {}
from_impl!(ToolchainError::Store(StoreError));
from_impl!(ToolchainError::Journal(JournalError));

/// `state.json` and `config`.
#[derive(Debug)]
pub enum StateError {
    Io {
        path: PathBuf,
        source: io::Error,
    },
    /// Unreadable JSON or a `format` this build does not know (E1830).
    Unreadable {
        path: PathBuf,
        why: String,
    },
    /// A config key or value that is not allowed (E1870).
    Usage(String),
    Journal(JournalError),
}

impl StateError {
    pub fn code(&self) -> &'static str {
        match self {
            StateError::Io { source, .. } => io_code(source, "E1835"),
            StateError::Unreadable { .. } => "E1830",
            StateError::Usage(_) => "E1870",
            StateError::Journal(e) => e.code(),
        }
    }
}

impl fmt::Display for StateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StateError::Io { path, source } => write!(f, "{}: {source}", path.display()),
            StateError::Unreadable { path, why } => {
                write!(f, "{} is unreadable or too new: {why}", path.display())
            }
            StateError::Usage(m) => f.write_str(m),
            StateError::Journal(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for StateError {}
from_impl!(StateError::Journal(JournalError));

/// Shims in `bin/`.
#[derive(Debug)]
pub enum ShimError {
    Io {
        path: PathBuf,
        source: io::Error,
    },
    Journal(JournalError),
    /// A shim name that is not a plain command name (from a manifest or a receipt).
    BadName(String),
}

impl ShimError {
    pub fn code(&self) -> &'static str {
        match self {
            ShimError::Io { source, .. } => io_code(source, "E1835"),
            ShimError::Journal(e) => e.code(),
            ShimError::BadName(_) => "E1825",
        }
    }
}

impl fmt::Display for ShimError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ShimError::Io { path, source } => write!(f, "{}: {source}", path.display()),
            ShimError::Journal(e) => e.fmt(f),
            ShimError::BadName(n) => write!(
                f,
                "the shim name {n:?} is not a plain command name; no file was made for it"
            ),
        }
    }
}

impl std::error::Error for ShimError {}
from_impl!(ShimError::Journal(JournalError));

/// Profile PATH blocks.
#[derive(Debug)]
pub enum PathError {
    Io { path: PathBuf, source: io::Error },
    Journal(JournalError),
}

impl PathError {
    pub fn code(&self) -> &'static str {
        match self {
            PathError::Io { source, .. } => io_code(source, "E1835"),
            PathError::Journal(e) => e.code(),
        }
    }
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PathError::Io { path, source } => write!(f, "{}: {source}", path.display()),
            PathError::Journal(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for PathError {}
from_impl!(PathError::Journal(JournalError));

/// The humidor mirror.
#[derive(Debug)]
pub enum HumidorError {
    Io {
        path: PathBuf,
        source: io::Error,
    },
    /// `humidor/index.json` exists but is not a JSON object (left alone).
    BadIndex {
        path: PathBuf,
        why: String,
    },
    /// The cig receipt could not be updated.
    Receipt(String),
    Journal(JournalError),
}

impl HumidorError {
    pub fn code(&self) -> &'static str {
        match self {
            HumidorError::Io { source, .. } => io_code(source, "E1835"),
            HumidorError::BadIndex { .. } | HumidorError::Receipt(_) => "E1830",
            HumidorError::Journal(e) => e.code(),
        }
    }
}

impl fmt::Display for HumidorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HumidorError::Io { path, source } => write!(f, "{}: {source}", path.display()),
            HumidorError::BadIndex { path, why } => {
                write!(f, "{} is unreadable: {why}", path.display())
            }
            HumidorError::Receipt(m) => write!(f, "cig receipt: {m}"),
            HumidorError::Journal(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for HumidorError {}
from_impl!(HumidorError::Journal(JournalError));

/// Helper: wrap an `io::Error` with its path.
pub(crate) fn jio(path: &std::path::Path) -> impl Fn(io::Error) -> JournalError + '_ {
    move |source| JournalError::Io {
        path: path.to_path_buf(),
        source,
    }
}
