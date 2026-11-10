// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! Throwaway TUF repositories for the attack tests. Every key is generated when a test runs, lives
//! in a temporary directory, and is gone when the test ends. No key is ever written into the tree.

use aws_lc_rs::digest::{digest, SHA256};
use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::Ed25519KeyPair;
use jiff::{SignedDuration, Timestamp};
use std::collections::HashMap;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use tough::editor::signed::{PathExists, SignedRole};
use tough::editor::RepositoryEditor;
use tough::key_source::{KeySource, LocalKeySource};
use tough::schema::decoded::{Decoded, Hex};
use tough::schema::key::Key;
use tough::schema::{Hashes, KeyHolder, RoleKeys, RoleType, Root, Target};
use tough::sign::{parse_keypair, Sign};
use tough::{ExpirationEnforcement, Repository, RepositoryLoader, TargetName};

pub fn nz(n: u64) -> NonZeroU64 {
    NonZeroU64::new(n).expect("non-zero")
}

pub fn days(n: i64) -> SignedDuration {
    SignedDuration::from_hours(n * 24)
}

/// An Ed25519 key kept in a file under a temporary directory.
#[derive(Debug, Clone)]
pub struct TestKey {
    pub path: PathBuf,
    pub key: Key,
    pub id: Decoded<Hex>,
}

impl TestKey {
    pub fn generate(dir: &Path, name: &str) -> Self {
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).expect("generate a key");
        let path = dir.join(format!("{name}.pk8"));
        std::fs::write(&path, pkcs8.as_ref()).expect("write the key");
        let key = parse_keypair(pkcs8.as_ref()).expect("parse").tuf_key();
        let id = key.key_id().expect("key id");
        Self { path, key, id }
    }
    pub fn source(&self) -> Box<dyn KeySource> {
        Box::new(LocalKeySource {
            path: self.path.clone(),
        })
    }
}

pub fn sources(keys: &[&TestKey]) -> Vec<Box<dyn KeySource>> {
    keys.iter().map(|k| k.source()).collect()
}

/// The keys of one repository: three offline root keys and one key for each other role.
#[derive(Debug, Clone)]
pub struct KeySet {
    pub root: Vec<TestKey>,
    pub targets: TestKey,
    pub snapshot: TestKey,
    pub timestamp: TestKey,
}

impl KeySet {
    pub fn generate(dir: &Path, tag: &str) -> Self {
        let k = |n: &str| TestKey::generate(dir, &format!("{tag}-{n}"));
        Self {
            root: vec![k("root-a"), k("root-b"), k("root-c")],
            targets: k("targets"),
            snapshot: k("snapshot"),
            timestamp: k("timestamp"),
        }
    }
    /// Every key, for the editor to choose from (it signs each role with the ones that role names).
    pub fn all(&self) -> Vec<Box<dyn KeySource>> {
        let mut v: Vec<Box<dyn KeySource>> = self.root.iter().map(TestKey::source).collect();
        v.push(self.targets.source());
        v.push(self.snapshot.source());
        v.push(self.timestamp.source());
        v
    }
}

fn role_keys(keys: &[&TestKey], threshold: u64) -> RoleKeys {
    RoleKeys {
        keyids: keys.iter().map(|k| k.id.clone()).collect(),
        threshold: nz(threshold),
        _extra: HashMap::new(),
    }
}

/// A root naming `root_keys` (with `root_threshold`) and one key for each other role.
pub fn build_root(
    version: u64,
    expires: Timestamp,
    root_keys: &[&TestKey],
    root_threshold: u64,
    targets: &TestKey,
    snapshot: &TestKey,
    timestamp: &TestKey,
) -> Root {
    let mut keys = HashMap::new();
    for k in root_keys
        .iter()
        .copied()
        .chain([targets, snapshot, timestamp])
    {
        keys.insert(k.id.clone(), k.key.clone());
    }
    let mut roles = HashMap::new();
    roles.insert(RoleType::Root, role_keys(root_keys, root_threshold));
    roles.insert(RoleType::Targets, role_keys(&[targets], 1));
    roles.insert(RoleType::Snapshot, role_keys(&[snapshot], 1));
    roles.insert(RoleType::Timestamp, role_keys(&[timestamp], 1));
    Root {
        spec_version: "1.0.0".into(),
        consistent_snapshot: true,
        version: nz(version),
        expires,
        keys,
        roles,
        _extra: HashMap::new(),
    }
}

/// Sign `root` with whichever of `signers` the root `holder` names for the root role.
/// A first root is its own holder. A rotation is signed twice: by the new root's keys (holder =
/// new root) and by the old root's (holder = old root), and the two signature lists are joined.
pub async fn sign_root(root: &Root, holder: &Root, signers: &[&TestKey]) -> SignedRole<Root> {
    SignedRole::new(
        root.clone(),
        &KeyHolder::Root(holder.clone()),
        &sources(signers),
        &SystemRandom::new(),
    )
    .await
    .expect("sign root")
}

/// A root rotation: the new root, signed by its own threshold and by the old root's threshold.
pub async fn rotate_root(
    new: &Root,
    new_signers: &[&TestKey],
    old: &Root,
    old_signers: &[&TestKey],
) -> SignedRole<Root> {
    if new_signers.is_empty() {
        // Nobody signed with a new key; `tough` refuses to sign with no matching key at all.
        return sign_root(new, old, old_signers).await;
    }
    let by_new = sign_root(new, new, new_signers).await;
    let by_old = sign_root(new, old, old_signers).await;
    by_new
        .add_old_signatures(by_old.signed().signatures.clone())
        .expect("join signatures")
}

/// One file to publish. `custom` is the signed free-form field of a target (the place a build's
/// attestation reference would go).
#[derive(Debug, Clone)]
pub struct File {
    pub name: String,
    pub bytes: Vec<u8>,
    pub custom: Option<serde_json::Value>,
}

impl File {
    pub fn new(name: &str, bytes: &[u8]) -> Self {
        Self {
            name: name.into(),
            bytes: bytes.to_vec(),
            custom: None,
        }
    }
    pub fn with_custom(mut self, v: serde_json::Value) -> Self {
        self.custom = Some(v);
        self
    }
    fn target(&self) -> Target {
        let mut custom = HashMap::new();
        if let Some(serde_json::Value::Object(m)) = &self.custom {
            for (k, v) in m {
                custom.insert(k.clone(), v.clone());
            }
        }
        Target {
            length: self.bytes.len() as u64,
            hashes: Hashes {
                sha256: digest(&SHA256, &self.bytes).as_ref().to_vec().into(),
                _extra: HashMap::new(),
            },
            custom,
            _extra: HashMap::new(),
        }
    }
}

/// What one release looks like: version numbers and expiries for each role.
#[derive(Debug, Clone)]
pub struct Release {
    pub targets_version: u64,
    pub snapshot_version: u64,
    pub timestamp_version: u64,
    pub targets_expires: Timestamp,
    pub snapshot_expires: Timestamp,
    pub timestamp_expires: Timestamp,
}

impl Release {
    /// All three roles at version `v`, expiring in 30, 7 and 1 days: the targets, snapshot and
    /// timestamp expiries decision 0004 proposes.
    pub fn at(v: u64) -> Self {
        let now = Timestamp::now();
        Self {
            targets_version: v,
            snapshot_version: v,
            timestamp_version: v,
            targets_expires: now + days(30),
            snapshot_expires: now + days(7),
            timestamp_expires: now + days(1),
        }
    }
}

/// A published repository: where a mirror would serve metadata and targets from.
#[derive(Debug, Clone)]
pub struct Published {
    pub meta: PathBuf,
    pub targets: PathBuf,
}

/// Write a root file where the editor can read it, publish `files` under `rel` signed with
/// `keys`, and lay metadata and (hash-prefixed, consistent-snapshot) targets out under `out`.
pub async fn publish(
    root: &SignedRole<Root>,
    keys: &[Box<dyn KeySource>],
    files: &[File],
    rel: &Release,
    out: &Path,
) -> Published {
    let meta = out.join("metadata");
    let targets = out.join("targets");
    let src = out.join("src");
    std::fs::create_dir_all(&src).expect("mkdir");
    root.write(&meta, true).await.expect("write root");
    let root_file = meta.join(format!("{}.root.json", root.signed().signed.version));

    let mut editor = RepositoryEditor::new(&root_file).await.expect("editor");
    editor
        .targets_version(nz(rel.targets_version))
        .expect("v")
        .targets_expires(rel.targets_expires)
        .expect("e")
        .snapshot_version(nz(rel.snapshot_version))
        .snapshot_expires(rel.snapshot_expires)
        .timestamp_version(nz(rel.timestamp_version))
        .timestamp_expires(rel.timestamp_expires);
    for f in files {
        std::fs::write(src.join(&f.name), &f.bytes).expect("write source file");
        editor
            .add_target(TargetName::new(&f.name).expect("name"), f.target())
            .expect("add target");
    }
    let signed = editor.sign(keys).await.expect("sign repository");
    signed.write(&meta).await.expect("write metadata");
    signed
        .copy_targets(&src, &targets, PathExists::Fail)
        .await
        .expect("copy targets");
    Published { meta, targets }
}

pub fn dir_url(p: &Path) -> url::Url {
    url::Url::from_directory_path(p).expect("absolute directory")
}

/// What a client does with a mirror: load with a trusted root and a datastore that remembers the
/// newest metadata it has seen (this is the rollback defense).
pub async fn load(
    trusted_root: &[u8],
    mirror: &Published,
    datastore: &Path,
    enforcement: ExpirationEnforcement,
) -> tough::error::Result<Repository> {
    std::fs::create_dir_all(datastore).expect("datastore directory");
    RepositoryLoader::new(
        &trusted_root,
        dir_url(&mirror.meta),
        dir_url(&mirror.targets),
    )
    .datastore(datastore)
    .expiration_enforcement(enforcement)
    .load()
    .await
}

/// Read a whole target through `tough`, which checks its length and SHA-256 as it streams.
pub async fn fetch(repo: &Repository, name: &str) -> Result<Option<Vec<u8>>, tough::error::Error> {
    use tough::IntoVec;
    let name = TargetName::new(name)?;
    match repo.read_target(&name).await? {
        Some(stream) => Ok(Some(stream.into_vec().await?)),
        None => Ok(None),
    }
}

pub fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("mkdir");
    for e in std::fs::read_dir(from).expect("read dir") {
        let e = e.expect("entry");
        let dest = to.join(e.file_name());
        if e.file_type().expect("type").is_dir() {
            copy_dir(&e.path(), &dest);
        } else {
            std::fs::copy(e.path(), dest).expect("copy");
        }
    }
}

pub fn copy_published(p: &Published, to: &Path) -> Published {
    let q = Published {
        meta: to.join("metadata"),
        targets: to.join("targets"),
    };
    copy_dir(&p.meta, &q.meta);
    copy_dir(&p.targets, &q.targets);
    q
}

/// A repository's first root, its keys and a place to publish from.
pub struct World {
    pub dir: tempfile::TempDir,
    pub keys: KeySet,
    pub root: Root,
    pub root_signed: SignedRole<Root>,
}

impl World {
    pub async fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let keys = KeySet::generate(dir.path(), "a");
        let root = build_root(
            1,
            Timestamp::now() + days(365),
            &keys.root.iter().collect::<Vec<_>>(),
            2,
            &keys.targets,
            &keys.snapshot,
            &keys.timestamp,
        );
        let root_signed = sign_root(&root, &root, &[&keys.root[0], &keys.root[1]]).await;
        Self {
            dir,
            keys,
            root,
            root_signed,
        }
    }
    /// The bytes a client ships with: the first root.
    pub fn trusted_root(&self) -> Vec<u8> {
        self.root_signed.buffer().clone()
    }
    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }
    pub async fn publish(&self, name: &str, files: &[File], rel: &Release) -> Published {
        publish(
            &self.root_signed,
            &self.keys.all(),
            files,
            rel,
            &self.path(name),
        )
        .await
    }
}
