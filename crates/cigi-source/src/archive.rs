// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0
//! Safe extraction of a `.tar.gz` into a staging directory (E1826 on anything unsafe).
//!
//! Refused: absolute paths, `..`, backslashes, symbolic and hard links of any kind (stricter than
//! section 2.3, which only names links that leave the directory: a link has no place in a receipt,
//! whose `files` are bytes), device, fifo and other special files, a file over 256 MB, more than
//! 20 000 entries, 1 GiB in all, the same path twice, a file that is also a directory.
//! Permissions are not taken from the archive: files are written plain, and `StagedFile::exec`
//! records only whether the owner-execute-or-any-execute bit was set; the store applies the modes.

use crate::error::ArchiveError;
use cigi_home::StagedFile;
use flate2::read::GzDecoder;
use std::collections::HashSet;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use tar::EntryType;

pub const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_ENTRIES: usize = 20_000;
pub const MAX_TOTAL_BYTES: u64 = 1024 * 1024 * 1024;

fn unsafe_entry(entry: &Path, why: &'static str) -> ArchiveError {
    ArchiveError::Unsafe {
        entry: entry.display().to_string(),
        why,
    }
}

/// Normalize an entry name to a relative path of plain components after removing `strip` of them.
/// `Ok(None)`: nothing left (the stripped top directory itself).
fn clean(name: &Path, strip: u32) -> Result<Option<PathBuf>, ArchiveError> {
    let mut parts: Vec<&std::ffi::OsStr> = Vec::new();
    for c in name.components() {
        match c {
            Component::Normal(s) => {
                if s.to_string_lossy().contains('\\') {
                    return Err(unsafe_entry(name, "a backslash in a name"));
                }
                parts.push(s)
            }
            Component::CurDir => {}
            Component::ParentDir => return Err(unsafe_entry(name, "a `..` component")),
            Component::RootDir | Component::Prefix(_) => {
                return Err(unsafe_entry(name, "an absolute path"))
            }
        }
    }
    let strip = strip as usize;
    if parts.len() <= strip {
        return Ok(None);
    }
    Ok(Some(parts[strip..].iter().collect()))
}

pub fn unpack_tar_gz(
    file: &Path,
    strip: u32,
    into: &Path,
) -> Result<Vec<StagedFile>, ArchiveError> {
    let io_at = |p: &Path| {
        let p = p.to_path_buf();
        move |source| ArchiveError::Io { path: p, source }
    };
    let f = std::fs::File::open(file).map_err(io_at(file))?;
    std::fs::create_dir_all(into).map_err(io_at(into))?;
    let mut ar = tar::Archive::new(GzDecoder::new(f));
    let unreadable = |e: std::io::Error| ArchiveError::Unreadable(e.to_string());
    let mut out: Vec<StagedFile> = Vec::new();
    let mut files: HashSet<PathBuf> = HashSet::new();
    let mut dirs: HashSet<PathBuf> = HashSet::new();
    let mut total = 0u64;
    let mut count = 0usize;
    for entry in ar.entries().map_err(unreadable)? {
        let mut entry = entry.map_err(unreadable)?;
        count += 1;
        if count > MAX_ENTRIES {
            return Err(ArchiveError::TooBig {
                why: format!("more than {MAX_ENTRIES} entries"),
            });
        }
        let raw = entry.path().map_err(unreadable)?.into_owned();
        let kind = entry.header().entry_type();
        match kind {
            EntryType::XGlobalHeader
            | EntryType::XHeader
            | EntryType::GNULongName
            | EntryType::GNULongLink => continue,
            EntryType::Symlink | EntryType::Link => {
                return Err(unsafe_entry(
                    &raw,
                    "links are not allowed in a toolchain archive",
                ))
            }
            EntryType::Regular | EntryType::Continuous | EntryType::Directory => {}
            _ => return Err(unsafe_entry(&raw, "a device, fifo or other special file")),
        }
        let Some(rel) = clean(&raw, strip)? else {
            if kind == EntryType::Directory {
                continue;
            }
            return Err(unsafe_entry(
                &raw,
                "a file outside the directory that is stripped",
            ));
        };
        // every ancestor must be a directory we know of (or are about to make), never a file
        for anc in rel
            .ancestors()
            .skip(1)
            .filter(|a| !a.as_os_str().is_empty())
        {
            if files.contains(anc) {
                return Err(unsafe_entry(&raw, "its parent was an earlier file"));
            }
        }
        let dest = into.join(&rel);
        if kind == EntryType::Directory {
            if files.contains(&rel) {
                return Err(unsafe_entry(
                    &raw,
                    "a directory with the name of an earlier file",
                ));
            }
            std::fs::create_dir_all(&dest).map_err(io_at(&dest))?;
            dirs.insert(rel);
            continue;
        }
        if files.contains(&rel) || dirs.contains(&rel) {
            return Err(unsafe_entry(&raw, "the same path twice"));
        }
        let size = entry.header().size().map_err(unreadable)?;
        if size > MAX_FILE_BYTES {
            return Err(ArchiveError::TooBig {
                why: format!("{} is {size} bytes (limit {MAX_FILE_BYTES})", raw.display()),
            });
        }
        total += size;
        if total > MAX_TOTAL_BYTES {
            return Err(ArchiveError::TooBig {
                why: format!("more than {MAX_TOTAL_BYTES} bytes in all"),
            });
        }
        let exec = entry
            .header()
            .mode()
            .map(|m| m & 0o111 != 0)
            .unwrap_or(false);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(io_at(parent))?;
        }
        let mut w = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&dest)
            .map_err(io_at(&dest))?;
        let mut buf = vec![0u8; 64 * 1024];
        let mut written = 0u64;
        let mut body = entry.by_ref().take(size);
        loop {
            let n = body.read(&mut buf).map_err(unreadable)?;
            if n == 0 {
                break;
            }
            w.write_all(&buf[..n]).map_err(io_at(&dest))?;
            written += n as u64;
        }
        if written != size {
            return Err(ArchiveError::Unreadable(format!(
                "{} ended after {written} of {size} bytes",
                raw.display()
            )));
        }
        // no fsync here: this is staging; `Store::put_file` syncs when the bytes enter the store.
        files.insert(rel.clone());
        // The store hashes what it keeps (`materialize`).
        out.push(StagedFile {
            path: rel,
            source: dest,
            exec,
        });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}
