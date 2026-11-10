// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! A listing of a directory tree with content hashes, for "the home is exactly as it was"
//! assertions (`a2_interrupted_install`, `a2_digest_attacks`, `no_real_home`).
//!
//! Portable by design: kinds, sizes, sha256 of file contents and the read-only flag. Unix mode
//! bits and inode numbers belong to `cigi-home::platform`, where `cfg(unix)` is allowed.

use crate::sha256_hex;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    Dir,
    File {
        sha256: String,
        size: u64,
        readonly: bool,
    },
    Symlink {
        target: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Snapshot {
    /// Relative path with forward slashes, sorted.
    pub entries: BTreeMap<String, Entry>,
}

impl Snapshot {
    /// Walk `root`. A missing root is an empty snapshot (so "before the first install" works).
    /// Symlinks are recorded, never followed.
    pub fn take(root: &Path) -> Snapshot {
        let mut s = Snapshot::default();
        walk(root, root, &mut s.entries);
        s
    }

    /// Entries whose path does not start with any of `prefixes` (for ignoring `runs/`, `tin/`...).
    pub fn without(&self, prefixes: &[&str]) -> Snapshot {
        Snapshot {
            entries: self
                .entries
                .iter()
                .filter(|(p, _)| {
                    !prefixes
                        .iter()
                        .any(|x| p.as_str() == *x || p.starts_with(&format!("{x}/")))
                })
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        }
    }

    /// Human-readable differences; empty when equal.
    pub fn diff(&self, other: &Snapshot) -> Vec<String> {
        let mut out = Vec::new();
        for (p, e) in &self.entries {
            match other.entries.get(p) {
                None => out.push(format!("removed: {p}")),
                Some(o) if o != e => out.push(format!("changed: {p}")),
                Some(_) => {}
            }
        }
        for p in other.entries.keys() {
            if !self.entries.contains_key(p) {
                out.push(format!("added: {p}"));
            }
        }
        out
    }

    /// Panics listing the differences, if any.
    pub fn assert_same(&self, other: &Snapshot) {
        let d = self.diff(other);
        assert!(d.is_empty(), "snapshots differ:\n  {}", d.join("\n  "));
    }

    /// One hash over the whole listing (the `no_real_home` check).
    pub fn listing_hash(&self) -> String {
        let mut text = String::new();
        for (p, e) in &self.entries {
            text.push_str(&format!("{p}\t{e:?}\n"));
        }
        sha256_hex(text.as_bytes())
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Entry>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut items: Vec<_> = rd.flatten().collect();
    items.sort_by_key(|e| e.file_name());
    for e in items {
        let path = e.path();
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            let target = std::fs::read_link(&path)
                .map(|t| t.display().to_string())
                .unwrap_or_default();
            out.insert(rel, Entry::Symlink { target });
        } else if meta.is_dir() {
            out.insert(rel, Entry::Dir);
            walk(root, &path, out);
        } else {
            let sha = std::fs::read(&path)
                .map(|b| sha256_hex(&b))
                .unwrap_or_default();
            out.insert(
                rel,
                Entry::File {
                    sha256: sha,
                    size: meta.len(),
                    readonly: meta.permissions().readonly(),
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_every_kind_of_change() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("a/b")).unwrap();
        std::fs::write(d.path().join("a/b/f"), b"one").unwrap();
        std::fs::write(d.path().join("g"), b"g").unwrap();
        let before = Snapshot::take(d.path());
        assert_eq!(before, Snapshot::take(d.path()));
        assert_eq!(
            before.listing_hash(),
            Snapshot::take(d.path()).listing_hash()
        );
        std::fs::write(d.path().join("a/b/f"), b"two").unwrap();
        std::fs::remove_file(d.path().join("g")).unwrap();
        std::fs::write(d.path().join("new"), b"n").unwrap();
        let after = Snapshot::take(d.path());
        let diff = before.diff(&after);
        assert!(diff.contains(&"changed: a/b/f".to_string()));
        assert!(diff.contains(&"removed: g".to_string()));
        assert!(diff.contains(&"added: new".to_string()));
        assert_ne!(before.listing_hash(), after.listing_hash());
    }

    #[test]
    fn missing_root_is_empty_and_prefixes_are_ignored() {
        assert!(Snapshot::take(Path::new("/nonexistent/cigi-testkit")).is_empty());
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("runs")).unwrap();
        std::fs::write(d.path().join("runs/x"), b"x").unwrap();
        std::fs::write(d.path().join("runs2"), b"y").unwrap();
        let s = Snapshot::take(d.path()).without(&["runs"]);
        assert!(s.entries.contains_key("runs2") && !s.entries.contains_key("runs/x"));
    }
}
