// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! Shared helpers for the integration tests. Scratch homes only.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use cig_compat::Version;
use cigi_home::*;

pub struct Fx {
    pub tmp: tempfile::TempDir,
    pub home: Home,
    pub lock: InstallerLock,
}

pub fn fx() -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let home = Home::at(tmp.path().join("u").join(".cigscript"));
    home.ensure_layout().unwrap();
    let lock = home.lock().unwrap();
    Fx { tmp, home, lock }
}

impl Fx {
    pub fn txn(&self, label: &str) -> Txn {
        Txn::begin(&self.home, label, &self.lock).unwrap()
    }

    /// Write a file under the scratch area and return its path.
    pub fn src(&self, rel: &str, body: &[u8]) -> PathBuf {
        let p = self.tmp.path().join("stage").join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, body).unwrap();
        p
    }

    pub fn staged(&self, files: &[(&str, &[u8], bool)]) -> Vec<StagedFile> {
        files
            .iter()
            .map(|(rel, body, exec)| StagedFile {
                path: PathBuf::from(rel),
                source: self.src(&format!("{}-{}", body.len(), rel.replace('/', "_")), body),
                exec: *exec,
            })
            .collect()
    }
}

pub fn v(s: &str) -> Version {
    Version::parse(s).unwrap()
}

pub fn prov() -> Provenance {
    Provenance {
        from_carton: Some("0.1.0".into()),
        source: ReceiptSource {
            repo: "otmof-ops/cig-orchestrator".into(),
            tag: "v1.17.0".into(),
            asset: "cig-orchestrator-v1.17.0.tar.gz".into(),
            origin: "github".into(),
        },
        artifact: ReceiptArtifact {
            sha256: "97d38b2b90e4110fbe20d9c2ee1b25f4fbd95e7bfeadce55cd06557dd0b45c3b".into(),
            size: 123456,
        },
        trust: ReceiptTrust {
            level: "checksummed".into(),
            independent: true,
            attested_by: None,
            manifest_sequence: 7,
        },
        license: None,
        shims: vec![ShimRef {
            name: "cigo".into(),
            path: "bin/cigo".into(),
        }],
    }
}

/// A tree listing (relative path, kind, content hash, mode) for
/// before/after equality: a rollback "to the byte" is compared on content,
/// not on length.
pub fn snapshot(root: &Path) -> Vec<String> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) {
        let mut ents: Vec<_> = match fs::read_dir(dir) {
            Ok(r) => r.flatten().collect(),
            Err(_) => return,
        };
        ents.sort_by_key(|e| e.file_name());
        for e in ents {
            let p = e.path();
            let rel = p.strip_prefix(base).unwrap().display().to_string();
            // transaction directories legitimately remain after a rollback
            if rel.starts_with("installer/txn/")
                || rel.starts_with("installer/lock")
                || rel == "installer.lock"
            {
                continue;
            }
            let md = fs::symlink_metadata(&p).unwrap();
            let mode = mode_of(&md);
            if md.file_type().is_symlink() {
                out.push(format!(
                    "l {rel} -> {}",
                    fs::read_link(&p).unwrap().display()
                ));
            } else if md.is_dir() {
                out.push(format!("d {rel} {mode}"));
                walk(base, &p, out);
            } else {
                use sha2::{Digest, Sha256};
                let body = fs::read(&p).unwrap();
                let sha = hex::encode(Sha256::digest(&body));
                out.push(format!("f {rel} {} {sha} {mode}", body.len()));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out
}

#[cfg(unix)]
fn mode_of(md: &fs::Metadata) -> String {
    use std::os::unix::fs::PermissionsExt;
    format!("{:o}", md.permissions().mode() & 0o7777)
}
#[cfg(not(unix))]
fn mode_of(_: &fs::Metadata) -> String {
    String::new()
}
