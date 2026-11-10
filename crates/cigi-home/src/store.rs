// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! The content-addressed store: `store/objects/<aa>/<sha256>[.x]` (section 4).

use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::error::StoreError;
use crate::home::Home;
use crate::platform;
use crate::util;

/// A stored file: its SHA-256 and whether it is the executable variant.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjectId {
    pub sha256: String,
    pub exec: bool,
}

impl ObjectId {
    pub fn new(sha256: impl Into<String>, exec: bool) -> ObjectId {
        ObjectId {
            sha256: sha256.into(),
            exec,
        }
    }

    /// `<sha256>` or `<sha256>.x`.
    pub fn file_name(&self) -> String {
        if self.exec {
            format!("{}.x", self.sha256)
        } else {
            self.sha256.clone()
        }
    }

    /// Inverse of [`ObjectId::file_name`]; `None` for anything else.
    pub fn parse_file_name(name: &str) -> Option<ObjectId> {
        let (hex, exec) = match name.strip_suffix(".x") {
            Some(h) => (h, true),
            None => (name, false),
        };
        let ok = hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        ok.then(|| ObjectId::new(hex, exec))
    }

    /// Mode of the stored file: 0444, or 0555 when executable.
    pub fn mode(&self) -> u32 {
        if self.exec {
            0o555
        } else {
            0o444
        }
    }
}

/// What the store holds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StoreUsage {
    pub objects: u64,
    pub bytes: u64,
}

/// A file copied into `store/tmp`, hashed, not yet visible as an object.
/// Publishing it is one rename. Dropping it without publishing leaves a temp
/// file that `recover` sweeps.
#[derive(Debug)]
pub struct StagedObject {
    tmp: PathBuf,
    id: ObjectId,
    bytes: u64,
}

impl StagedObject {
    pub fn id(&self) -> &ObjectId {
        &self.id
    }

    pub fn bytes(&self) -> u64 {
        self.bytes
    }
}

/// The store of one home.
#[derive(Debug, Clone, Copy)]
pub struct Store<'a> {
    home: &'a Home,
}

impl<'a> Store<'a> {
    pub fn new(home: &'a Home) -> Store<'a> {
        Store { home }
    }

    fn objects(&self) -> PathBuf {
        self.home.store().join("objects")
    }

    fn tmp(&self) -> PathBuf {
        self.home.store().join("tmp")
    }

    /// Where an object lives (whether or not it exists).
    pub fn object_path(&self, id: &ObjectId) -> PathBuf {
        let shard = id.sha256.get(..2).unwrap_or("00");
        self.objects().join(shard).join(id.file_name())
    }

    /// Copy `source` into `store/tmp` while hashing it, fsync, set the object's
    /// mode. Nothing is visible in `objects/` yet.
    pub fn stage_file(&self, source: &Path, exec: bool) -> Result<StagedObject, StoreError> {
        self.stage(source, exec, true)
    }

    /// As [`Store::stage_file`] without the per-file fsync: for a batch whose
    /// owner (`Txn`) fsyncs every published object before the transaction can
    /// commit. Until then the object is merely unproven, never referenced.
    pub fn stage_file_batched(
        &self,
        source: &Path,
        exec: bool,
    ) -> Result<StagedObject, StoreError> {
        self.stage(source, exec, false)
    }

    fn stage(&self, source: &Path, exec: bool, durable: bool) -> Result<StagedObject, StoreError> {
        let io = |p: &Path| {
            let p = p.to_path_buf();
            move |source| StoreError::Io {
                path: p.clone(),
                source,
            }
        };
        let tmp_dir = self.tmp();
        fs::create_dir_all(&tmp_dir).map_err(io(&tmp_dir))?;
        let tmp = tmp_dir.join(format!("{}.tmp", util::unique_suffix()));
        let res = (|| -> Result<(String, u64), StoreError> {
            let mut src = File::open(source).map_err(io(source))?;
            let mut dst = File::create(&tmp).map_err(io(&tmp))?;
            let mut h = Sha256::new();
            let mut buf = vec![0u8; 64 * 1024];
            let mut n = 0u64;
            loop {
                let r = src.read(&mut buf).map_err(io(source))?;
                if r == 0 {
                    break;
                }
                h.update(&buf[..r]);
                dst.write_all(&buf[..r]).map_err(io(&tmp))?;
                n += r as u64;
            }
            if durable {
                dst.sync_all().map_err(io(&tmp))?;
            }
            drop(dst);
            let id = ObjectId::new(hex::encode(h.finalize()), exec);
            platform::set_mode(&tmp, id.mode()).map_err(io(&tmp))?;
            Ok((id.sha256, n))
        })();
        match res {
            Ok((sha, bytes)) => Ok(StagedObject {
                tmp,
                id: ObjectId::new(sha, exec),
                bytes,
            }),
            Err(e) => {
                let _ = fs::remove_file(&tmp);
                Err(e)
            }
        }
    }

    /// Make a staged object visible (atomic rename). An existing object that
    /// verifies is kept and the staged copy dropped; one that does not verify
    /// is replaced (this is how `repair` heals the store).
    pub fn publish(&self, staged: StagedObject) -> Result<ObjectId, StoreError> {
        let id = staged.id.clone();
        let dest = self.object_path(&id);
        if self.verify(&id).is_ok() {
            let _ = fs::remove_file(&staged.tmp);
            return Ok(id);
        }
        let io = |p: &Path| {
            let p = p.to_path_buf();
            move |source| StoreError::Io {
                path: p.clone(),
                source,
            }
        };
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(io(parent))?;
        }
        if let Err(e) = fs::rename(&staged.tmp, &dest) {
            let _ = fs::remove_file(&staged.tmp);
            return Err(io(&dest)(e));
        }
        if let Some(parent) = dest.parent() {
            let _ = platform::sync_dir(parent);
        }
        Ok(id)
    }

    /// Copy a file in. Verifies nothing about the bytes (callers verify
    /// first). Atomic: temp, fsync, rename. Mode 0444 (0555 if `exec`).
    pub fn put_file(&self, bytes_path: &Path, exec: bool) -> Result<ObjectId, StoreError> {
        let staged = self.stage_file(bytes_path, exec)?;
        self.publish(staged)
    }

    pub fn has(&self, id: &ObjectId) -> bool {
        self.object_path(id).is_file()
    }

    /// The bytes hash to the name and the mode is the one the name says.
    pub fn verify(&self, id: &ObjectId) -> Result<(), StoreError> {
        let p = self.object_path(id);
        if !p.is_file() {
            return Err(StoreError::Missing {
                object: id.file_name(),
            });
        }
        let (actual, _) = util::sha256_file(&p).map_err(|source| StoreError::Io {
            path: p.clone(),
            source,
        })?;
        if actual != id.sha256 {
            return Err(StoreError::Mismatch {
                object: id.file_name(),
                actual,
            });
        }
        if let Some(m) = platform::mode_of(&p) {
            if m != id.mode() {
                return Err(StoreError::Mode {
                    object: id.file_name(),
                });
            }
        }
        Ok(())
    }

    /// Every object on disk with its size.
    pub fn all(&self) -> Vec<(ObjectId, u64)> {
        let mut out = Vec::new();
        let Ok(shards) = fs::read_dir(self.objects()) else {
            return out;
        };
        for shard in shards.flatten() {
            let Ok(files) = fs::read_dir(shard.path()) else {
                continue;
            };
            for f in files.flatten() {
                let name = f.file_name().to_string_lossy().into_owned();
                if let (Some(id), Ok(md)) = (ObjectId::parse_file_name(&name), f.metadata()) {
                    out.push((id, md.len()));
                }
            }
        }
        out.sort();
        out
    }

    /// Objects not in `keep`, with their sizes (what `gc` would free).
    pub fn unreferenced(&self, keep: &HashSet<ObjectId>) -> Vec<(ObjectId, u64)> {
        self.all()
            .into_iter()
            .filter(|(id, _)| !keep.contains(id))
            .collect()
    }

    pub fn du(&self) -> StoreUsage {
        let all = self.all();
        StoreUsage {
            objects: all.len() as u64,
            bytes: all.iter().map(|(_, n)| n).sum(),
        }
    }

    /// Delete one object (gc). Not journaled: only unreferenced objects.
    pub fn remove(&self, id: &ObjectId) -> Result<(), StoreError> {
        let p = self.object_path(id);
        util::remove_any(&p).map_err(|source| StoreError::Io {
            path: p.clone(),
            source,
        })?;
        if let Some(shard) = p.parent() {
            let _ = fs::remove_dir(shard); // only succeeds when empty
        }
        Ok(())
    }

    /// Delete everything in `store/tmp` (abandoned stagings). The caller holds
    /// the lock.
    pub fn sweep_tmp(&self) {
        if let Ok(rd) = fs::read_dir(self.tmp()) {
            for e in rd.flatten() {
                let _ = util::remove_any(&e.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> (tempfile::TempDir, Home) {
        let t = tempfile::tempdir().unwrap();
        let h = Home::at(t.path().join(".cigscript"));
        h.ensure_layout().unwrap();
        (t, h)
    }

    fn src(t: &Path, name: &str, body: &[u8]) -> PathBuf {
        let p = t.join(name);
        fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn put_verify_modes_and_layout() {
        let (t, h) = home();
        let s = h.object_store();
        let f = src(t.path(), "a", b"hello");
        let id = s.put_file(&f, false).unwrap();
        assert_eq!(id.sha256, util::sha256_bytes(b"hello"));
        assert!(s.has(&id));
        s.verify(&id).unwrap();
        let p = s.object_path(&id);
        assert!(p.ends_with(format!("objects/{}/{}", &id.sha256[..2], id.sha256)));
        if let Some(m) = platform::mode_of(&p) {
            assert_eq!(m, 0o444);
        }
        let idx = s.put_file(&f, true).unwrap();
        assert_ne!(id, idx);
        assert!(s.object_path(&idx).to_string_lossy().ends_with(".x"));
        if let Some(m) = platform::mode_of(&s.object_path(&idx)) {
            assert_eq!(m, 0o555);
        }
        s.verify(&idx).unwrap();
    }

    #[test]
    fn dedup_and_usage() {
        let (t, h) = home();
        let s = h.object_store();
        let a = src(t.path(), "a", b"same");
        let b = src(t.path(), "b", b"same");
        let i1 = s.put_file(&a, false).unwrap();
        let i2 = s.put_file(&b, false).unwrap();
        assert_eq!(i1, i2);
        assert_eq!(
            s.du(),
            StoreUsage {
                objects: 1,
                bytes: 4
            }
        );
        assert!(fs::read_dir(h.store().join("tmp"))
            .unwrap()
            .next()
            .is_none());
    }

    #[test]
    fn corruption_is_found_and_healed_by_put() {
        let (t, h) = home();
        let s = h.object_store();
        let f = src(t.path(), "a", b"payload");
        let id = s.put_file(&f, false).unwrap();
        let p = s.object_path(&id);
        platform::set_mode(&p, 0o644).unwrap();
        fs::write(&p, b"paYload").unwrap();
        assert!(matches!(s.verify(&id), Err(StoreError::Mismatch { .. })));
        s.put_file(&f, false).unwrap();
        s.verify(&id).unwrap();
        fs::remove_file(&p).unwrap();
        assert!(matches!(s.verify(&id), Err(StoreError::Missing { .. })));
    }

    #[test]
    fn wrong_mode_is_reported() {
        let (t, h) = home();
        let s = h.object_store();
        let id = s.put_file(&src(t.path(), "a", b"x"), false).unwrap();
        if platform::mode_of(&s.object_path(&id)).is_some() {
            platform::set_mode(&s.object_path(&id), 0o644).unwrap();
            assert!(matches!(s.verify(&id), Err(StoreError::Mode { .. })));
        }
    }

    #[test]
    fn unreferenced_lists_the_rest() {
        let (t, h) = home();
        let s = h.object_store();
        let a = s.put_file(&src(t.path(), "a", b"1"), false).unwrap();
        let b = s.put_file(&src(t.path(), "b", b"22"), true).unwrap();
        let keep: HashSet<ObjectId> = [a.clone()].into();
        assert_eq!(s.unreferenced(&keep), vec![(b.clone(), 2)]);
        s.remove(&b).unwrap();
        assert!(s.unreferenced(&keep).is_empty());
    }

    #[test]
    fn name_parsing() {
        let sha = "a".repeat(64);
        assert_eq!(
            ObjectId::parse_file_name(&sha),
            Some(ObjectId::new(sha.clone(), false))
        );
        assert_eq!(
            ObjectId::parse_file_name(&format!("{sha}.x")),
            Some(ObjectId::new(sha, true))
        );
        assert_eq!(ObjectId::parse_file_name("zz"), None);
        assert_eq!(ObjectId::parse_file_name(&"A".repeat(64)), None);
    }
}
