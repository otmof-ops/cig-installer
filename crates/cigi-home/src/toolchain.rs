// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0

//! Toolchains: read-only trees of hard links into the store, with a receipt
//! (sections 4.2, 4.3, 5.4).

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use cig_compat::Version;

use crate::error::ToolchainError;
use crate::home::Home;
use crate::journal::Txn;
use crate::store::ObjectId;
use crate::util;

/// File name of the receipt inside a toolchain directory.
pub const RECEIPT_NAME: &str = ".cigi-receipt.json";

/// Receipt format this build writes and reads.
pub const RECEIPT_FORMAT: u32 = 1;

/// A file prepared by the source layer, waiting to become part of a toolchain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedFile {
    /// Path inside the toolchain, relative, `/`-separated (`bin/cigo`).
    pub path: PathBuf,
    /// Where the bytes are right now (a staging directory).
    pub source: PathBuf,
    /// Whether the file is executable.
    pub exec: bool,
}

/// Where a build came from (receipt `source`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptSource {
    pub repo: String,
    pub tag: String,
    pub asset: String,
    pub origin: String,
}

/// The downloaded artifact (receipt `artifact`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptArtifact {
    pub sha256: String,
    pub size: u64,
}

/// How the artifact was trusted (receipt `trust`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptTrust {
    /// `pinned`, `attested` or `checksummed`.
    pub level: String,
    pub independent: bool,
    pub attested_by: Option<String>,
    pub manifest_sequence: u64,
}

/// One file of a toolchain (receipt `files`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptFile {
    pub path: String,
    pub sha256: String,
    pub exec: bool,
    pub bytes: u64,
}

/// A shim name and where its target is inside the toolchain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShimRef {
    pub name: String,
    pub path: String,
}

/// Everything about an install that the receipt records and `materialize`
/// cannot work out for itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provenance {
    pub from_carton: Option<String>,
    pub source: ReceiptSource,
    pub artifact: ReceiptArtifact,
    pub trust: ReceiptTrust,
    pub shims: Vec<ShimRef>,
}

/// `.cigi-receipt.json` (5.4). Field order is the file's key order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    pub format: u32,
    pub tool: String,
    pub version: String,
    pub installed_at: String,
    pub from_carton: Option<String>,
    pub txn: String,
    pub source: ReceiptSource,
    pub artifact: ReceiptArtifact,
    pub trust: ReceiptTrust,
    pub files: Vec<ReceiptFile>,
    pub shims: Vec<ShimRef>,
    /// `null`, `"mirrored"` or `"skipped"` (4.4).
    pub humidor_mirror: Option<String>,
}

impl Receipt {
    /// Pretty JSON, two-space indent, trailing newline.
    pub fn to_json(&self) -> String {
        let mut s = serde_json::to_string_pretty(self).unwrap_or_default();
        s.push('\n');
        s
    }

    /// The store objects this toolchain is made of.
    pub fn objects(&self) -> Vec<ObjectId> {
        self.files
            .iter()
            .map(|f| ObjectId::new(f.sha256.clone(), f.exec))
            .collect()
    }

    /// Path of a shim target inside the toolchain: the receipt's `shims` entry
    /// for `name`, else `bin/<name>`.
    pub fn shim_target(&self, name: &str) -> String {
        self.shims
            .iter()
            .find(|s| s.name == name)
            .map(|s| s.path.clone())
            .unwrap_or_else(|| format!("bin/{name}"))
    }
}

/// One way an installed toolchain differs from its receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Damage {
    /// No readable receipt: the directory is not trusted at all.
    NoReceipt,
    Missing {
        path: String,
    },
    Changed {
        path: String,
        expected: String,
        actual: String,
    },
    /// Right bytes, wrong executable bit.
    Mode {
        path: String,
    },
    /// A file the receipt does not list (or a symlink).
    Extra {
        path: String,
    },
}

impl Damage {
    pub fn path(&self) -> Option<&str> {
        match self {
            Damage::NoReceipt => None,
            Damage::Missing { path }
            | Damage::Changed { path, .. }
            | Damage::Mode { path }
            | Damage::Extra { path } => Some(path),
        }
    }
}

fn io_err(path: &Path) -> impl Fn(std::io::Error) -> ToolchainError + '_ {
    move |source| ToolchainError::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn clean_rel(p: &Path) -> Result<String, ToolchainError> {
    let bad = || ToolchainError::BadPath(p.display().to_string());
    let mut parts = Vec::new();
    for c in p.components() {
        match c {
            Component::Normal(s) => parts.push(s.to_str().ok_or_else(bad)?.to_string()),
            _ => return Err(bad()),
        }
    }
    if parts.is_empty() || parts.join("/") == RECEIPT_NAME {
        return Err(bad());
    }
    Ok(parts.join("/"))
}

/// Read and check `.cigi-receipt.json` of an installed toolchain.
pub fn read_receipt(home: &Home, tool: &str, v: &Version) -> Result<Receipt, ToolchainError> {
    read_receipt_at(&home.toolchain(tool, v), tool, &v.to_string())
}

fn read_receipt_at(dir: &Path, tool: &str, version: &str) -> Result<Receipt, ToolchainError> {
    let p = dir.join(RECEIPT_NAME);
    let text = match fs::read_to_string(&p) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(ToolchainError::NoReceipt {
                tool: tool.to_string(),
                version: version.to_string(),
            })
        }
        Err(e) => return Err(io_err(&p)(e)),
    };
    let bad = |why: String| ToolchainError::BadReceipt {
        path: p.clone(),
        why,
    };
    let val: serde_json::Value = serde_json::from_str(&text).map_err(|e| bad(e.to_string()))?;
    let found = val.get("format").and_then(|f| f.as_u64()).unwrap_or(0);
    if found == 0 || found > RECEIPT_FORMAT as u64 {
        return Err(bad(format!(
            "format {found}, this build knows {RECEIPT_FORMAT}"
        )));
    }
    serde_json::from_value(val).map_err(|e| bad(e.to_string()))
}

/// Build `toolchains/<tool>/<version>/` from staged files, inside `txn`:
/// each file goes into the store, then a partial directory of hard links plus
/// the receipt (written last) is renamed into place. A toolchain that is
/// already installed and verifies is left alone and its receipt returned
/// (idempotent). One that exists but is damaged or has no receipt is moved
/// aside (journaled) and rebuilt.
pub fn materialize(
    txn: &mut Txn,
    home: &Home,
    tool: &str,
    v: &Version,
    files: &[StagedFile],
    provenance: Provenance,
) -> Result<Receipt, ToolchainError> {
    let final_dir = home.toolchain(tool, v);
    if let Ok(r) = read_receipt(home, tool, v) {
        if verify_toolchain(home, tool, v).is_empty() {
            return Ok(r);
        }
    }
    // Validate the paths before touching anything.
    let mut seen = BTreeSet::new();
    let mut staged: Vec<(String, &StagedFile)> = Vec::with_capacity(files.len());
    for f in files {
        let rel = clean_rel(&f.path)?;
        if !seen.insert(rel.clone()) {
            return Err(ToolchainError::BadPath(format!("{rel} appears twice")));
        }
        staged.push((rel, f));
    }
    staged.sort_by(|a, b| a.0.cmp(&b.0));

    let tool_dir = home.toolchains().join(tool);
    let partial = tool_dir.join(format!(".{v}.partial-{}", txn.id()));
    txn.mkdir_all(&tool_dir)?;
    txn.mkdir(&partial)?;

    let store = home.object_store();
    let mut entries = Vec::with_capacity(staged.len());
    for (rel, f) in &staged {
        let id = txn.store_file(&store, &f.source, f.exec)?;
        let bytes = fs::metadata(store.object_path(&id))
            .map_err(io_err(&f.source))?
            .len();
        let dest = partial.join(rel);
        if let Some(parent) = dest.parent() {
            // journaled, with explicit modes: never the umask's group-write
            txn.mkdir_all(parent)?;
        }
        txn.link(&store.object_path(&id), &dest)?;
        entries.push(ReceiptFile {
            path: rel.clone(),
            sha256: id.sha256,
            exec: id.exec,
            bytes,
        });
    }

    let receipt = Receipt {
        format: RECEIPT_FORMAT,
        tool: tool.to_string(),
        version: v.to_string(),
        installed_at: util::now_rfc3339(),
        from_carton: provenance.from_carton,
        txn: txn.id().to_string(),
        source: provenance.source,
        artifact: provenance.artifact,
        trust: provenance.trust,
        files: entries,
        shims: provenance.shims,
        humidor_mirror: None,
    };
    let rpath = partial.join(RECEIPT_NAME);
    util::write_atomic(&rpath, receipt.to_json().as_bytes(), Some(0o644))
        .map_err(io_err(&rpath))?;
    // The replacement is complete: the swap is the last step, two renames
    // with nothing slow between them.
    if util::exists_any(&final_dir) {
        txn.delete(&final_dir)?;
    }
    txn.move_path(&partial, &final_dir)?;
    Ok(receipt)
}

/// Check an installed toolchain against its receipt: missing, changed
/// (bytes or executable bit) and extra files. Reads everything; repairs
/// nothing.
pub fn verify_toolchain(home: &Home, tool: &str, v: &Version) -> Vec<Damage> {
    let dir = home.toolchain(tool, v);
    let receipt = match read_receipt_at(&dir, tool, &v.to_string()) {
        Ok(r) => r,
        Err(_) => return vec![Damage::NoReceipt],
    };
    let mut out = Vec::new();
    let mut listed = BTreeSet::new();
    for f in &receipt.files {
        listed.insert(f.path.clone());
        let p = dir.join(&f.path);
        match fs::symlink_metadata(&p) {
            Err(_) => out.push(Damage::Missing {
                path: f.path.clone(),
            }),
            Ok(md) if !md.is_file() => out.push(Damage::Extra {
                path: f.path.clone(),
            }),
            Ok(_) => match util::sha256_file(&p) {
                Err(_) => out.push(Damage::Missing {
                    path: f.path.clone(),
                }),
                Ok((actual, _)) if actual != f.sha256 => out.push(Damage::Changed {
                    path: f.path.clone(),
                    expected: f.sha256.clone(),
                    actual,
                }),
                Ok(_) => {
                    if let Some(m) = crate::platform::mode_of(&p) {
                        if (m & 0o111 != 0) != f.exec {
                            out.push(Damage::Mode {
                                path: f.path.clone(),
                            });
                        }
                    }
                }
            },
        }
    }
    let mut stack = vec![(dir.clone(), String::new())];
    while let Some((d, prefix)) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let rel = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                stack.push((e.path(), rel));
            } else if rel != RECEIPT_NAME && !listed.contains(&rel) {
                out.push(Damage::Extra { path: rel });
            }
        }
    }
    out.sort_by(|a, b| a.path().cmp(&b.path()));
    out
}

/// Every installed toolchain: directories with a readable receipt, sorted by
/// tool then version. Half-built (`.partial-`) directories never count.
pub fn installed(home: &Home) -> Vec<(String, Version)> {
    let mut out = Vec::new();
    let Ok(tools) = fs::read_dir(home.toolchains()) else {
        return out;
    };
    for t in tools.flatten() {
        let tool = t.file_name().to_string_lossy().into_owned();
        if tool.starts_with('.') {
            continue;
        }
        let Ok(vs) = fs::read_dir(t.path()) else {
            continue;
        };
        for v in vs.flatten() {
            let name = v.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let Ok(ver) = Version::parse(&name) else {
                continue;
            };
            if read_receipt_at(&v.path(), &tool, &name).is_ok() {
                out.push((tool.clone(), ver));
            }
        }
    }
    out.sort();
    out
}

/// The objects that must survive `gc`: those of every installed toolchain,
/// those `bin/` links to (the shim and installer objects), and every object
/// of a toolchain directory whose receipt cannot be read (kept by inode: a
/// file linked from there is kept, whatever the receipt would have said).
/// The values say who needs each object; `bin/<name>` entries carry version
/// `0.0.0`. Where the platform has no inodes, an unreadable receipt keeps
/// the whole store.
pub fn referenced_objects(home: &Home) -> BTreeMap<ObjectId, Vec<(String, Version)>> {
    let mut m: BTreeMap<ObjectId, Vec<(String, Version)>> = BTreeMap::new();
    for (tool, v) in installed(home) {
        if let Ok(r) = read_receipt(home, &tool, &v) {
            for id in r.objects() {
                m.entry(id).or_default().push((tool.clone(), v.clone()));
            }
        }
    }
    // Who links to the store by inode: bin/ and toolchains we cannot read.
    let mut by_inode: BTreeMap<u64, Vec<(String, Version)>> = BTreeMap::new();
    let mut keep_all: Option<(String, Version)> = None;
    let zero = Version::parse("0.0.0").ok();
    let mut note_file = |p: &Path, who: (String, Version)| match crate::platform::ino(p) {
        Some(i) => by_inode.entry(i).or_default().push(who),
        None => keep_all = Some(who),
    };
    if let (Ok(rd), Some(zero)) = (fs::read_dir(home.bin()), zero.as_ref()) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            note_file(&e.path(), (format!("bin/{name}"), zero.clone()));
        }
    }
    if let Ok(tools) = fs::read_dir(home.toolchains()) {
        for t in tools.flatten() {
            let tool = t.file_name().to_string_lossy().into_owned();
            let Ok(vs) = fs::read_dir(t.path()) else {
                continue;
            };
            for v in vs.flatten() {
                let name = v.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') || read_receipt_at(&v.path(), &tool, &name).is_ok() {
                    continue;
                }
                let ver = Version::parse(&name)
                    .or_else(|_| Version::parse("0.0.0"))
                    .ok();
                let Some(ver) = ver else { continue };
                let mut stack = vec![v.path()];
                while let Some(d) = stack.pop() {
                    let Ok(rd) = fs::read_dir(&d) else { continue };
                    for e in rd.flatten() {
                        match e.file_type() {
                            Ok(ft) if ft.is_dir() => stack.push(e.path()),
                            Ok(_) => note_file(&e.path(), (tool.clone(), ver.clone())),
                            Err(_) => {}
                        }
                    }
                }
            }
        }
    }
    if by_inode.is_empty() && keep_all.is_none() {
        return m;
    }
    let store = home.object_store();
    for (id, _) in store.all() {
        if let Some(who) = &keep_all {
            m.entry(id).or_default().push(who.clone());
        } else if let Some(i) = crate::platform::ino(&store.object_path(&id)) {
            if let Some(whos) = by_inode.get(&i) {
                m.entry(id).or_default().extend(whos.iter().cloned());
            }
        }
    }
    m
}

/// The names come from `state.json` and from directory listings: only a tool name the manifest
/// rules allow and a version that is plain text may reach a path.
fn check_toolchain_name(tool: &str, v: &Version) -> Result<(), ToolchainError> {
    let version = v.to_string();
    if !crate::shims::shim_name_ok(tool) || version.contains(['/', '\\']) || version.contains("..")
    {
        return Err(ToolchainError::BadPath(format!(
            "{tool} {version} is not a toolchain name"
        )));
    }
    Ok(())
}

/// Remove a toolchain directory outside any transaction, after the transaction that stopped
/// using it has committed (`gc`): best effort, idempotent, same name rules as
/// [`remove_toolchain`]. Nothing runs from it any more, so a crash before this leaves only a
/// directory the next `gc` removes.
pub fn remove_toolchain_now(home: &Home, tool: &str, v: &Version) -> Result<(), ToolchainError> {
    check_toolchain_name(tool, v)?;
    let dir = home.toolchain(tool, v);
    match fs::remove_dir_all(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(io_err(&dir)(e)),
    }
    let _ = fs::remove_dir(home.toolchains().join(tool)); // only succeeds when empty
    Ok(())
}

/// Remove a whole toolchain (journaled: it is moved into the txn directory,
/// so a rollback puts it back byte for byte).
pub fn remove_toolchain(
    txn: &mut Txn,
    home: &Home,
    tool: &str,
    v: &Version,
) -> Result<(), ToolchainError> {
    check_toolchain_name(tool, v)?;
    txn.delete(&home.toolchain(tool, v))?;
    Ok(())
}

/// Put one file of an installed toolchain back from the store (`repair`).
/// The damaged file is saved in the journal first. The object must verify.
pub fn restore_file(
    txn: &mut Txn,
    home: &Home,
    tool: &str,
    v: &Version,
    file: &ReceiptFile,
) -> Result<(), ToolchainError> {
    let store = home.object_store();
    let id = ObjectId::new(file.sha256.clone(), file.exec);
    store.verify(&id)?;
    let dest = home.toolchain(tool, v).join(&file.path);
    if let Some(parent) = dest.parent() {
        txn.mkdir_all(parent)?;
    }
    txn.link(&store.object_path(&id), &dest)?;
    Ok(())
}

/// Remove a file the receipt does not list (`repair`), journaled.
pub fn remove_extra(
    txn: &mut Txn,
    home: &Home,
    tool: &str,
    v: &Version,
    rel: &str,
) -> Result<(), ToolchainError> {
    let rel = clean_rel(Path::new(rel))?;
    txn.delete(&home.toolchain(tool, v).join(rel))?;
    Ok(())
}

/// Rewrite the receipt of an installed toolchain through the journal.
pub fn write_receipt(txn: &mut Txn, home: &Home, receipt: &Receipt) -> Result<(), ToolchainError> {
    let v = Version::parse(&receipt.version).map_err(|e| ToolchainError::BadPath(e.to_string()))?;
    let p: PathBuf = home.toolchain(&receipt.tool, &v).join(RECEIPT_NAME);
    txn.write(&p, receipt.to_json().as_bytes(), Some(0o644))?;
    Ok(())
}
