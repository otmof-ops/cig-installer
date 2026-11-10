// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! Offline bundles (#16): one `.tar` holding a flat mirror directory and a small index.
//!
//! The layout inside is exactly what a mirror directory holds: `manifest-<channel>.toml`, its
//! `.sha256`, and every asset with its `.sha256` by file name, plus `bundle.json`. That is why a
//! bundle needs no trust code of its own: `unpack_bundle` lays the files out, and `BundleSource`
//! serves them like any other mirror, so a bundle is admitted by the same rules as one (one
//! source, `pinned` or refused). The index is for people and for early damage detection; nothing
//! is trusted because the index says so.
//!
//! The file is written to be reproducible: entries sorted by name, modification time 0, owner 0,
//! mode 0644, index first. The same inputs give the same bytes, so a second run can be compared.
//!
//! Unpacking refuses everything but plain files with plain names (no directories, links, devices,
//! pax or long-name tricks, duplicates), over 4096 entries, a file over 1 GiB or 8 GiB in all.
//! Bundles are unsigned until #9 (TUF) lands.

use crate::digest::random_suffix;
use crate::http::{self, MAX_DOWNLOAD};
use crate::location::{is_plain_segment, Location};
use crate::progress::Progress;
use crate::source::{FetchInfo, MirrorSource, Source};
use crate::FetchError;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tar::EntryType;

/// The index's file name inside a bundle.
pub const INDEX_NAME: &str = "bundle.json";
pub const MAX_ENTRIES: usize = 4096;
pub const MAX_TOTAL_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_INDEX_BYTES: u64 = 1024 * 1024;
const MAX_NAME_BYTES: usize = 200;

/// Why a bundle could not be read (E1837) or written (E1839).
#[derive(Debug)]
pub enum BundleError {
    /// Not a readable bundle: not a tar, no index, an index that does not parse.
    Unreadable(String),
    /// An entry that is not a plain file with a plain name.
    Unsafe { entry: String, why: &'static str },
    /// A limit was passed.
    TooBig { why: String },
    /// A file and the index disagree, or a file the index does not list.
    Mismatch { name: String, why: String },
    /// Reading the bundle or unpacking it failed on the disk.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The bundle could not be written.
    Write { path: PathBuf, why: String },
}

impl BundleError {
    pub fn code(&self) -> &'static str {
        match self {
            BundleError::Io { source, .. } if source.raw_os_error() == Some(28) => "E1836",
            BundleError::Write { why, .. } if why.contains("No space left") => "E1836",
            BundleError::Write { .. } => "E1839",
            _ => "E1837",
        }
    }
}

impl fmt::Display for BundleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BundleError::Unreadable(why) => write!(f, "the bundle could not be read: {why}"),
            BundleError::Unsafe { entry, why } => {
                write!(f, "the bundle's entry {entry:?} was refused: {why}")
            }
            BundleError::TooBig { why } => write!(f, "the bundle was refused: {why}"),
            BundleError::Mismatch { name, why } => {
                write!(f, "{name} does not match the bundle's index: {why}")
            }
            BundleError::Io { path, source } => {
                write!(f, "could not use {}: {source}", path.display())
            }
            BundleError::Write { path, why } => {
                write!(f, "could not write the bundle {}: {why}", path.display())
            }
        }
    }
}
impl std::error::Error for BundleError {}

// ---------------------------------------------------------------- writing

/// Where a file's bytes come from.
#[derive(Debug, Clone)]
pub enum BundleData {
    Bytes(Vec<u8>),
    File(PathBuf),
}

/// One file of a bundle. `sha256` and `size` are what the caller admitted; the writer checks the
/// bytes against them as it copies, so nothing but what was admitted goes in.
#[derive(Debug, Clone)]
pub struct BundleFile {
    pub name: String,
    pub data: BundleData,
    pub sha256: String,
    pub size: u64,
}

impl BundleFile {
    pub fn bytes(name: impl Into<String>, bytes: Vec<u8>) -> BundleFile {
        BundleFile {
            name: name.into(),
            sha256: crate::digest::sha256_hex(&bytes),
            size: bytes.len() as u64,
            data: BundleData::Bytes(bytes),
        }
    }
}

struct Checked<'a, R: Read> {
    inner: R,
    hash: &'a mut Sha256,
    seen: &'a mut u64,
}
impl<R: Read> Read for Checked<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.hash.update(&buf[..n]);
        *self.seen += n as u64;
        Ok(n)
    }
}

fn header_for(size: u64) -> tar::Header {
    let mut h = tar::Header::new_gnu();
    h.set_size(size);
    h.set_mode(0o644);
    h.set_uid(0);
    h.set_gid(0);
    h.set_mtime(0);
    h.set_entry_type(EntryType::Regular);
    h
}

/// Write `files` (and an index of them, first) to `out`, through a partial file that is renamed
/// into place only when complete. `meta` is merged into the index (`channel`, `carton`, ...);
/// `format`, `signed` and `files` are the writer's own.
pub fn write_bundle(
    out: &Path,
    meta: &[(&str, serde_json::Value)],
    mut files: Vec<BundleFile>,
) -> Result<(), BundleError> {
    let werr = |why: String| BundleError::Write {
        path: out.to_path_buf(),
        why,
    };
    files.sort_by(|a, b| a.name.cmp(&b.name));
    for w in files.windows(2) {
        if w[0].name == w[1].name {
            return Err(werr(format!("{} would be in it twice", w[0].name)));
        }
    }
    for f in &files {
        if !is_plain_segment(&f.name) || f.name == INDEX_NAME || f.name.len() > MAX_NAME_BYTES {
            return Err(werr(format!(
                "{:?} is not a name a bundle can hold",
                f.name
            )));
        }
    }
    if files.len() + 1 > MAX_ENTRIES {
        return Err(werr(format!("more than {MAX_ENTRIES} files")));
    }
    let mut index = serde_json::Map::new();
    index.insert("format".into(), 1.into());
    for (k, v) in meta {
        index.insert((*k).to_string(), v.clone());
    }
    index.insert("signed".into(), false.into());
    index.insert(
        "files".into(),
        files
            .iter()
            .map(|f| serde_json::json!({"name": f.name, "size": f.size, "sha256": f.sha256}))
            .collect::<Vec<_>>()
            .into(),
    );
    let mut index_text = serde_json::to_string_pretty(&serde_json::Value::Object(index))
        .map_err(|e| werr(e.to_string()))?;
    index_text.push('\n');

    let name = out
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let partial = out.with_file_name(format!(".{name}.partial-{}", random_suffix()));
    let result = (|| -> Result<(), BundleError> {
        let file = http::create_private(&partial).map_err(|e| werr(e.to_string()))?;
        let mut b = tar::Builder::new(file);
        let mut h = header_for(index_text.len() as u64);
        b.append_data(&mut h, INDEX_NAME, index_text.as_bytes())
            .map_err(|e| werr(e.to_string()))?;
        for f in &files {
            let mut hash = Sha256::new();
            let mut seen = 0u64;
            let mut h = header_for(f.size);
            match &f.data {
                BundleData::Bytes(bytes) => {
                    let r = Checked {
                        inner: bytes.as_slice(),
                        hash: &mut hash,
                        seen: &mut seen,
                    };
                    b.append_data(&mut h, &f.name, r)
                        .map_err(|e| werr(e.to_string()))?;
                }
                BundleData::File(path) => {
                    let mut src = std::fs::File::open(path)
                        .map_err(|e| werr(format!("{}: {e}", path.display())))?;
                    {
                        let r = Checked {
                            inner: Read::by_ref(&mut src).take(f.size),
                            hash: &mut hash,
                            seen: &mut seen,
                        };
                        b.append_data(&mut h, &f.name, r)
                            .map_err(|e| werr(e.to_string()))?;
                    }
                    let mut extra = [0u8; 1];
                    if src.read(&mut extra).map_err(|e| werr(e.to_string()))? != 0 {
                        return Err(werr(format!("{} is longer than what was admitted", f.name)));
                    }
                }
            }
            if seen != f.size || hex::encode(hash.finalize()) != f.sha256 {
                return Err(werr(format!(
                    "{} is not the bytes that were admitted",
                    f.name
                )));
            }
        }
        let file = b.into_inner().map_err(|e| werr(e.to_string()))?;
        file.sync_all().map_err(|e| werr(e.to_string()))?;
        drop(file);
        std::fs::rename(&partial, out).map_err(|e| werr(e.to_string()))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    result
}

// ---------------------------------------------------------------- reading

/// What `unpack_bundle` found.
#[derive(Debug, Clone)]
pub struct Unpacked {
    pub index: serde_json::Value,
    /// `(name, size)` of every file but the index, sorted by name.
    pub files: Vec<(String, u64)>,
}

impl Unpacked {
    /// The channels with a manifest in the bundle (`manifest-<channel>.toml`).
    pub fn channels(&self) -> Vec<String> {
        self.files
            .iter()
            .filter_map(|(n, _)| {
                n.strip_prefix("manifest-")
                    .and_then(|r| r.strip_suffix(".toml"))
                    .map(String::from)
            })
            .collect()
    }
}

fn io_at(path: &Path) -> impl FnOnce(std::io::Error) -> BundleError + '_ {
    move |source| BundleError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Unpack `file` into `dest`, an existing empty directory the caller owns (and removes). Plain
/// files only, each created new and owner-only; the index is checked against what came out.
pub fn unpack_bundle(file: &Path, dest: &Path) -> Result<Unpacked, BundleError> {
    let f = std::fs::File::open(file).map_err(io_at(file))?;
    let len = f.metadata().map_err(io_at(file))?.len();
    if len > MAX_TOTAL_BYTES + 64 * 1024 * 1024 {
        return Err(BundleError::TooBig {
            why: format!("the file is {len} bytes"),
        });
    }
    let mut ar = tar::Archive::new(f);
    let entries = ar
        .entries()
        .map_err(|e| BundleError::Unreadable(e.to_string()))?;
    let mut seen: BTreeMap<String, (u64, String)> = BTreeMap::new();
    let mut total = 0u64;
    let mut count = 0usize;
    for entry in entries {
        let mut entry = entry.map_err(|e| BundleError::Unreadable(e.to_string()))?;
        count += 1;
        if count > MAX_ENTRIES {
            return Err(BundleError::TooBig {
                why: format!("more than {MAX_ENTRIES} entries"),
            });
        }
        let raw = entry.path_bytes().into_owned();
        let name = String::from_utf8_lossy(&raw).into_owned();
        let bad = |why: &'static str| BundleError::Unsafe {
            entry: name.clone(),
            why,
        };
        if entry.header().entry_type() != EntryType::Regular {
            return Err(bad("not a plain file"));
        }
        if matches!(entry.pax_extensions(), Ok(Some(_)) | Err(_)) {
            return Err(bad("a pax header"));
        }
        if raw.len() > MAX_NAME_BYTES || !is_plain_segment(&name) {
            return Err(bad("not a plain file name"));
        }
        if seen.contains_key(&name) {
            return Err(bad("in the bundle twice"));
        }
        let size = entry.size();
        let limit = if name == INDEX_NAME {
            MAX_INDEX_BYTES
        } else {
            MAX_DOWNLOAD
        };
        if size > limit {
            return Err(BundleError::TooBig {
                why: format!("{name} is {size} bytes"),
            });
        }
        total += size;
        if total > MAX_TOTAL_BYTES {
            return Err(BundleError::TooBig {
                why: format!("more than {MAX_TOTAL_BYTES} bytes in all"),
            });
        }
        let path = dest.join(&name);
        let mut out = http::create_private(&path).map_err(io_at(&path))?;
        let mut hash = Sha256::new();
        let mut buf = vec![0u8; 64 * 1024];
        let mut n_total = 0u64;
        loop {
            let n = entry
                .read(&mut buf)
                .map_err(|e| BundleError::Unreadable(e.to_string()))?;
            if n == 0 {
                break;
            }
            hash.update(&buf[..n]);
            out.write_all(&buf[..n]).map_err(io_at(&path))?;
            n_total += n as u64;
        }
        if n_total != size {
            return Err(BundleError::Unreadable(format!("{name} is cut short")));
        }
        out.sync_all().map_err(io_at(&path))?;
        seen.insert(name, (size, hex::encode(hash.finalize())));
    }
    let index_path = dest.join(INDEX_NAME);
    if !seen.contains_key(INDEX_NAME) {
        return Err(BundleError::Unreadable(format!("it has no {INDEX_NAME}")));
    }
    let text = std::fs::read_to_string(&index_path).map_err(io_at(&index_path))?;
    let index: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| BundleError::Unreadable(format!("{INDEX_NAME}: {e}")))?;
    if index.get("format").and_then(|v| v.as_u64()) != Some(1) {
        return Err(BundleError::Unreadable(format!(
            "{INDEX_NAME} is not format 1"
        )));
    }
    let rows = index
        .get("files")
        .and_then(|v| v.as_array())
        .ok_or_else(|| BundleError::Unreadable(format!("{INDEX_NAME} lists no files")))?;
    let mut listed: BTreeMap<&str, (u64, &str)> = BTreeMap::new();
    for r in rows {
        let (Some(n), Some(s), Some(h)) = (
            r.get("name").and_then(|v| v.as_str()),
            r.get("size").and_then(|v| v.as_u64()),
            r.get("sha256").and_then(|v| v.as_str()),
        ) else {
            return Err(BundleError::Unreadable(format!(
                "{INDEX_NAME} has a row without a name, size and sha256"
            )));
        };
        listed.insert(n, (s, h));
    }
    for (name, (size, sha)) in &seen {
        if name == INDEX_NAME {
            continue;
        }
        match listed.get(name.as_str()) {
            None => {
                return Err(BundleError::Mismatch {
                    name: name.clone(),
                    why: "the index does not list it".into(),
                })
            }
            Some((s, h)) if s != size || *h != sha => {
                return Err(BundleError::Mismatch {
                    name: name.clone(),
                    why: "its size or sha256 differs from the index".into(),
                })
            }
            Some(_) => {}
        }
    }
    for name in listed.keys() {
        if !seen.contains_key(*name) {
            return Err(BundleError::Mismatch {
                name: (*name).to_string(),
                why: "the index lists it and the bundle does not hold it".into(),
            });
        }
    }
    let files = seen
        .into_iter()
        .filter(|(n, _)| n != INDEX_NAME)
        .map(|(n, (s, _))| (n, s))
        .collect();
    Ok(Unpacked { index, files })
}

// ---------------------------------------------------------------- the source

/// An unpacked bundle served as a mirror. One source: the manifest, the checksums and the files
/// all came from the same file, so it is never `independent` and the single-source rule applies.
#[derive(Debug, Clone)]
pub struct BundleSource {
    inner: MirrorSource,
    label: String,
}

impl BundleSource {
    /// `dir` is where `unpack_bundle` put the files; `label` names the bundle file in receipts.
    pub fn new(dir: &Path, label: &str) -> Result<BundleSource, FetchError> {
        let abs = std::fs::canonicalize(dir).map_err(|source| FetchError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        Ok(BundleSource {
            inner: MirrorSource::parse(&abs.to_string_lossy())?,
            label: label.to_string(),
        })
    }
}

impl Source for BundleSource {
    fn name(&self) -> String {
        format!("bundle:{}", self.label)
    }
    fn independent(&self) -> bool {
        false
    }
    fn fetch(
        &self,
        loc: &Location,
        dest: &Path,
        progress: &mut dyn Progress,
    ) -> Result<FetchInfo, FetchError> {
        self.inner.fetch(loc, dest, progress)
    }
    fn fetch_text(&self, loc: &Location, max_bytes: usize) -> Result<String, FetchError> {
        self.inner.fetch_text(loc, max_bytes)
    }
}
