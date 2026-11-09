// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0

//! Small shared helpers: hashing, atomic writes, ids, UTC time.

use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use time::OffsetDateTime;

use crate::platform;

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Hex SHA-256 of a byte slice.
pub(crate) fn sha256_bytes(b: &[u8]) -> String {
    hex::encode(Sha256::digest(b))
}

/// Hex SHA-256 and length of a file, streamed.
pub(crate) fn sha256_file(p: &Path) -> io::Result<(String, u64)> {
    let mut f = File::open(p)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut n = 0u64;
    loop {
        let r = f.read(&mut buf)?;
        if r == 0 {
            break;
        }
        h.update(&buf[..r]);
        n += r as u64;
    }
    Ok((hex::encode(h.finalize()), n))
}

/// A name part that is unique within this process and very likely across them.
pub(crate) fn unique_suffix() -> String {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = OffsetDateTime::now_utc().unix_timestamp_nanos();
    format!(
        "{}-{}-{:x}",
        std::process::id(),
        n,
        nanos & 0xffff_ffff_ffff
    )
}

fn civil(t: OffsetDateTime) -> (i32, u8, u8, u8, u8, u8) {
    (
        t.year(),
        u8::from(t.month()),
        t.day(),
        t.hour(),
        t.minute(),
        t.second(),
    )
}

/// Now, UTC, RFC 3339, whole seconds: `2026-11-12T10:03:11Z`.
pub fn now_rfc3339() -> String {
    let (y, mo, d, h, mi, s) = civil(OffsetDateTime::now_utc());
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// A transaction id: `YYYYMMDDTHHMMSS-<6 hex>`.
pub(crate) fn new_txn_id() -> String {
    let (y, mo, d, h, mi, s) = civil(OffsetDateTime::now_utc());
    let seed = format!("{}-{}", unique_suffix(), std::process::id());
    let digest = sha256_bytes(seed.as_bytes());
    format!("{y:04}{mo:02}{d:02}T{h:02}{mi:02}{s:02}-{}", &digest[..6])
}

/// Hard link, falling back to a copy when linking is impossible (other
/// filesystem, FAT, ...). An existing target is an error, never overwritten.
pub(crate) fn link_or_copy(from: &Path, to: &Path) -> io::Result<()> {
    match fs::hard_link(from, to) {
        Ok(()) => Ok(()),
        Err(e)
            if e.kind() == io::ErrorKind::AlreadyExists || e.kind() == io::ErrorKind::NotFound =>
        {
            Err(e)
        }
        Err(_) => fs::copy(from, to).map(|_| ()),
    }
}

/// A sibling temp path for an atomic write.
pub(crate) fn temp_sibling(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!(".{name}.tmp-{}", unique_suffix()))
}

/// Write to a sibling temp name, fsync, rename. `mode` is applied before the
/// rename (default: the platform's umask-governed creation mode).
pub(crate) fn write_atomic(path: &Path, bytes: &[u8], mode: Option<u32>) -> io::Result<()> {
    let tmp = temp_sibling(path);
    let res = (|| {
        let mut f = File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        if let Some(m) = mode {
            platform::set_mode(&tmp, m)?;
        }
        fs::rename(&tmp, path)
    })();
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
        return res;
    }
    if let Some(parent) = path.parent() {
        let _ = platform::sync_dir(parent);
    }
    Ok(())
}

/// Read a file as UTF-8 text, `None` when it does not exist.
pub(crate) fn read_opt(path: &Path) -> io::Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Remove a file or a directory tree; absent is fine.
pub(crate) fn remove_any(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(md) if md.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Whether anything (file, dir, dangling link) is at the path.
pub(crate) fn exists_any(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}
