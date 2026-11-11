// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `cargo xtask tuf ...`: write and sign the update channel's metadata (decision 0004).
//!
//! Keys are read from files whose paths the maintainer gives; nothing here reads a key from the
//! repository, and nothing prints one. Roles and who signs them:
//!
//! | role | key | when |
//! |---|---|---|
//! | `root` | threshold of the offline root keys | `init`, `rotate` |
//! | `targets` | the offline targets key | `init` (delegates to `releases` once), `sign targets`, `rotate` |
//! | `releases` | online releases key | every release: the manifest and its builds |
//! | `snapshot` | online snapshot key | every change, and weekly |
//! | `timestamp` | online timestamp key | daily |
//!
//! A release therefore needs only the online keys. A stolen releases key can publish files inside
//! the delegation's paths and nothing else; the offline `targets` key signs the delegation, not
//! each release.
//!
//! Files are written under consistent-snapshot names: `N.root.json`, `N.targets.json`,
//! `N.releases.json`, `N.snapshot.json`, `timestamp.json`.

use aws_lc_rs::digest::{digest, SHA256};
use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::Ed25519KeyPair;
use cig_compat::Manifest;
use jiff::{SignedDuration, Timestamp};
use std::collections::HashMap;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use tough::editor::signed::SignedRole;
use tough::key_source::KeySource;
use tough::schema::decoded::{Decoded, Hex};
use tough::schema::key::Key;
use tough::schema::{
    DelegatedRole, DelegatedTargets, Delegations, Hashes, KeyHolder, Metafile, PathPattern,
    PathSet, RoleKeys, RoleType, Root, Signed, Snapshot, Target, Targets,
};
use tough::sign::{parse_keypair, Sign};
use tough::{async_trait, TargetName};

pub const SPEC: &str = "1.0.0";
/// The name of the online delegated role that signs the manifest and its builds.
pub const RELEASES: &str = "releases";

/// Expiries from decision 0004, section 2, as amended: the targets role now signs only the
/// delegation, so it is re-signed with the root, not with every carton.
pub const ROOT_DAYS: i64 = 365;
pub const TARGETS_DAYS: i64 = 365;
pub const RELEASES_DAYS: i64 = 30;
pub const SNAPSHOT_DAYS: i64 = 30;
pub const TIMESTAMP_DAYS: i64 = 7;

/// What the `releases` role may sign. A new product is a new line here, signed by the offline
/// targets key at the next sitting: that is the point of the delegation.
pub const RELEASE_PATHS: &[&str] = &[
    "manifest-*.toml",
    "cig-*",
    "cigo-*",
    "cigcompile-*",
    "cigar-*",
    "ciginstaller-*",
    "install.sh",
    "install.ps1",
];

type Res<T> = Result<T, String>;

fn e<E: std::fmt::Display>(what: &str) -> impl FnOnce(E) -> String + '_ {
    move |x| format!("{what}: {x}")
}

fn nz(n: u64) -> NonZeroU64 {
    NonZeroU64::new(n.max(1)).unwrap_or(NonZeroU64::MIN)
}

pub fn days(n: i64) -> SignedDuration {
    SignedDuration::from_hours(n * 24)
}

fn run<F: std::future::Future>(f: F) -> Res<F::Output> {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .map_err(e("runtime"))
        .map(|rt| rt.block_on(f))
}

/// A private key held in memory, read from a file the maintainer named.
#[derive(Debug, Clone)]
pub struct KeyFile {
    der: Vec<u8>,
}

impl KeyFile {
    /// Reads a PKCS#8 Ed25519 key: raw DER, or its base64 text (the form a GitHub secret holds).
    pub fn read(path: &Path) -> Res<KeyFile> {
        let raw = std::fs::read(path).map_err(e(&format!("{}", path.display())))?;
        if Ed25519KeyPair::from_pkcs8(&raw).is_ok() {
            return Ok(KeyFile { der: raw });
        }
        let text: String = String::from_utf8_lossy(&raw)
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        let der = b64_decode(&text).ok_or_else(|| {
            format!(
                "{} is not an Ed25519 PKCS#8 key (DER or base64)",
                path.display()
            )
        })?;
        Ed25519KeyPair::from_pkcs8(&der)
            .map_err(|_| format!("{} is not an Ed25519 PKCS#8 key", path.display()))?;
        Ok(KeyFile { der })
    }
    /// The PKCS#8 bytes, for code that builds a `KeySource` of its own (tests).
    pub fn der(&self) -> &[u8] {
        &self.der
    }
    pub fn public(&self) -> Res<Key> {
        Ok(parse_keypair(&self.der).map_err(e("key"))?.tuf_key())
    }
    pub fn id(&self) -> Res<Decoded<Hex>> {
        self.public()?.key_id().map_err(e("key id"))
    }
    fn source(&self) -> Box<dyn KeySource> {
        Box::new(MemKey(self.der.clone()))
    }
}

struct MemKey(Vec<u8>);
impl std::fmt::Debug for MemKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MemKey(..)")
    }
}
#[async_trait]
impl KeySource for MemKey {
    async fn as_sign(
        &self,
    ) -> Result<Box<dyn Sign>, Box<dyn std::error::Error + Send + Sync + 'static>> {
        Ok(Box::new(parse_keypair(&self.0)?))
    }
    async fn write(
        &self,
        _: &str,
        _: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
        Err("a key is never written back".into())
    }
}

fn sources(keys: &[KeyFile]) -> Vec<Box<dyn KeySource>> {
    keys.iter().map(KeyFile::source).collect()
}

fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.trim_end_matches('=').bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

fn b64_encode(b: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                s.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

/// Makes a new Ed25519 key at `path` (0600, never overwriting) and returns its key id. With
/// `base64` the file holds the base64 text of the key, the form to paste into a GitHub secret.
pub fn keygen(path: &Path, base64: bool) -> Res<String> {
    let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).map_err(|_| "no key")?;
    let bytes = if base64 {
        let mut t = b64_encode(pkcs8.as_ref()).into_bytes();
        t.push(b'\n');
        t
    } else {
        pkcs8.as_ref().to_vec()
    };
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    use std::io::Write;
    o.open(path)
        .and_then(|mut f| f.write_all(&bytes))
        .map_err(e(&format!("{}", path.display())))?;
    let id = KeyFile::read(path)?.id()?;
    Ok(hex::encode(&*id))
}

fn role_keys(keys: &[&KeyFile], threshold: u64) -> Res<RoleKeys> {
    let mut ids = Vec::new();
    for k in keys {
        ids.push(k.id()?);
    }
    Ok(RoleKeys {
        keyids: ids,
        threshold: nz(threshold),
        _extra: HashMap::new(),
    })
}

/// A root signed by `signers` under `holder`'s notion of who may sign it. It does not check the
/// threshold; `rotate` does, and tests use this to make roots that clients must refuse.
pub fn sign_root(root: &Root, holder: &Root, signers: &[KeyFile]) -> Res<SignedRole<Root>> {
    run(SignedRole::new(
        root.clone(),
        &KeyHolder::Root(holder.clone()),
        &sources(signers),
        &SystemRandom::new(),
    ))?
    .map_err(e("sign root"))
}

/// The new root signed by the new root's keys and by the old root's, the two lists joined.
pub fn cross_sign(
    new: &Root,
    old: &Root,
    new_signers: &[KeyFile],
    old_signers: &[KeyFile],
) -> Res<SignedRole<Root>> {
    let by_new = sign_root(new, new, new_signers)?;
    let by_old = sign_root(new, old, old_signers)?;
    by_new
        .add_old_signatures(by_old.signed().signatures.clone())
        .map_err(e("join signatures"))
}

fn valid_signatures(r: &SignedRole<Root>, holder: &Root) -> usize {
    let ids = holder
        .roles
        .get(&RoleType::Root)
        .map(|k| k.keyids.clone())
        .unwrap_or_default();
    r.signed()
        .signatures
        .iter()
        .filter(|s| ids.contains(&s.keyid))
        .count()
}

fn write_file(dir: &Path, name: &str, bytes: &[u8]) -> Res<()> {
    std::fs::create_dir_all(dir).map_err(e("create the metadata directory"))?;
    std::fs::write(dir.join(name), bytes).map_err(e(name))
}

/// The newest `N.<role>.json` in `dir`, as (N, bytes).
fn latest(dir: &Path, role: &str) -> Res<Option<(u64, Vec<u8>)>> {
    let suffix = format!(".{role}.json");
    let mut best: Option<u64> = None;
    let rd = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(_) => return Ok(None),
    };
    for ent in rd.flatten() {
        let n = ent.file_name().to_string_lossy().to_string();
        if let Some(v) = n.strip_suffix(&suffix).and_then(|v| v.parse::<u64>().ok()) {
            best = Some(best.map_or(v, |b| b.max(v)));
        }
    }
    match best {
        Some(v) => {
            let b = std::fs::read(dir.join(format!("{v}{suffix}"))).map_err(e(&suffix))?;
            Ok(Some((v, b)))
        }
        None => Ok(None),
    }
}

fn need<T>(o: Option<T>, what: &str) -> Res<T> {
    o.ok_or_else(|| format!("there is no {what} in the metadata directory; run `tuf init` first"))
}

fn latest_root(dir: &Path) -> Res<(u64, Root)> {
    let (v, b) = need(latest(dir, "root")?, "root")?;
    let s: Signed<Root> = serde_json::from_slice(&b).map_err(e("root.json"))?;
    Ok((v, s.signed))
}

fn latest_targets(dir: &Path) -> Res<(u64, Targets)> {
    let (v, b) = need(latest(dir, "targets")?, "targets")?;
    let s: Signed<Targets> = serde_json::from_slice(&b).map_err(e("targets.json"))?;
    Ok((v, s.signed))
}

fn delegations_of(t: &Targets) -> Res<Delegations> {
    t.delegations
        .clone()
        .ok_or_else(|| "the targets role has no delegations".to_string())
}

fn expiry(now: Timestamp, d: i64) -> Timestamp {
    now + days(d)
}

pub struct Init<'a> {
    pub dir: &'a Path,
    pub root_keys: Vec<KeyFile>,
    pub root_threshold: u64,
    pub targets_key: KeyFile,
    pub releases_key: KeyFile,
    pub snapshot_key: KeyFile,
    pub timestamp_key: KeyFile,
    pub now: Timestamp,
}

/// The ceremony: root version 1 signed by the root keys, and targets version 1 signed by the
/// targets key, delegating the manifest and its builds to the online releases key.
pub fn init(a: &Init) -> Res<()> {
    if latest(a.dir, "root")?.is_some() {
        return Err("a root already exists there; use `tuf rotate`".into());
    }
    if a.root_threshold == 0 || a.root_threshold as usize > a.root_keys.len() {
        return Err("the root threshold must be 1 to the number of root keys".into());
    }
    let rk: Vec<&KeyFile> = a.root_keys.iter().collect();
    let mut keys = HashMap::new();
    for k in rk
        .iter()
        .copied()
        .chain([&a.targets_key, &a.snapshot_key, &a.timestamp_key])
    {
        keys.insert(k.id()?, k.public()?);
    }
    let mut roles = HashMap::new();
    roles.insert(RoleType::Root, role_keys(&rk, a.root_threshold)?);
    roles.insert(RoleType::Targets, role_keys(&[&a.targets_key], 1)?);
    roles.insert(RoleType::Snapshot, role_keys(&[&a.snapshot_key], 1)?);
    roles.insert(RoleType::Timestamp, role_keys(&[&a.timestamp_key], 1)?);
    let root = Root {
        spec_version: SPEC.into(),
        consistent_snapshot: true,
        version: nz(1),
        expires: expiry(a.now, ROOT_DAYS),
        keys,
        roles,
        _extra: HashMap::new(),
    };
    let signed = cross_sign(&root, &root, &a.root_keys, &a.root_keys)?;
    if valid_signatures(&signed, &root) < a.root_threshold as usize {
        return Err("too few root keys were given to meet the threshold".into());
    }
    write_file(a.dir, "1.root.json", signed.buffer())?;

    let mut t = Targets::new(SPEC.into(), nz(1), expiry(a.now, TARGETS_DAYS));
    let mut d = Delegations::new();
    d.keys
        .insert(a.releases_key.id()?, a.releases_key.public()?);
    let mut paths = Vec::new();
    for p in RELEASE_PATHS {
        paths.push(PathPattern::new(*p).map_err(e("path"))?);
    }
    d.roles.push(DelegatedRole {
        name: RELEASES.into(),
        keyids: vec![a.releases_key.id()?],
        threshold: nz(1),
        paths: PathSet::Paths(paths),
        terminating: true,
        targets: None,
    });
    t.delegations = Some(d);
    let st = run(SignedRole::new(
        t,
        &KeyHolder::Root(root),
        &sources(std::slice::from_ref(&a.targets_key)),
        &SystemRandom::new(),
    ))?
    .map_err(e("sign targets"))?;
    write_file(a.dir, "1.targets.json", st.buffer())
}

/// Re-signs the top-level targets (new version and expiry), keeping its delegations. Offline,
/// at the same sitting as the root's refresh (every 180 days).
pub fn sign_targets(dir: &Path, key: &KeyFile, now: Timestamp, days_: i64) -> Res<u64> {
    let (_, root) = latest_root(dir)?;
    let (v, mut t) = latest_targets(dir)?;
    t.version = nz(v + 1);
    t.expires = expiry(now, days_);
    let st = run(SignedRole::new(
        t,
        &KeyHolder::Root(root),
        &sources(std::slice::from_ref(key)),
        &SystemRandom::new(),
    ))?
    .map_err(e("sign targets"))?;
    write_file(dir, &format!("{}.targets.json", v + 1), st.buffer())?;
    Ok(v + 1)
}

/// One file the `releases` role vouches for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub length: u64,
    pub sha256: String,
    pub custom: serde_json::Map<String, serde_json::Value>,
}

impl Entry {
    pub fn of_bytes(name: &str, bytes: &[u8]) -> Entry {
        Entry {
            name: name.into(),
            length: bytes.len() as u64,
            sha256: hex::encode(digest(&SHA256, bytes).as_ref()),
            custom: Default::default(),
        }
    }
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({"name": self.name, "length": self.length, "sha256": self.sha256, "custom": self.custom})
    }
    pub fn from_json(v: &serde_json::Value) -> Res<Entry> {
        let s = |k: &str| {
            v.get(k)
                .and_then(|x| x.as_str())
                .map(str::to_string)
                .ok_or_else(|| format!("an entry has no `{k}`"))
        };
        Ok(Entry {
            name: s("name")?,
            length: v
                .get("length")
                .and_then(|x| x.as_u64())
                .ok_or("an entry has no `length`")?,
            sha256: s("sha256")?,
            custom: v
                .get("custom")
                .and_then(|x| x.as_object())
                .cloned()
                .unwrap_or_default(),
        })
    }
    fn target(&self) -> Res<Target> {
        let sha = hex::decode(&self.sha256).map_err(e("sha256"))?;
        Ok(Target {
            length: self.length,
            hashes: Hashes {
                sha256: sha.into(),
                _extra: HashMap::new(),
            },
            custom: self.custom.clone().into_iter().collect(),
            _extra: HashMap::new(),
        })
    }
}

/// The signed targets `manifest assemble` hands to `tuf sign releases`: the manifest document
/// (`manifest-<channel>.toml`) and one entry per build it names, carrying the build's attestation
/// reference in `custom` so the installer checks it at install exactly as `Attest` does.
pub fn entries_for(manifest_text: &str) -> Res<Vec<Entry>> {
    let m = Manifest::from_str(manifest_text).map_err(|x| x.to_string())?;
    let mut out = vec![Entry::of_bytes(
        &format!("manifest-{}.toml", m.channel),
        manifest_text.as_bytes(),
    )];
    let mut seen = std::collections::BTreeSet::new();
    for b in m.builds.values() {
        for art in b.artifacts.values() {
            let name = cigi_source::pins::key(&art.tool, &art.version.to_string(), &art.target);
            if !seen.insert(name.clone()) {
                continue;
            }
            let mut custom = serde_json::Map::new();
            if art.attests {
                custom.insert("repo".into(), art.repo.clone().into());
                custom.insert("attests".into(), true.into());
            }
            out.push(Entry {
                name,
                length: art.size,
                sha256: art.sha256.clone(),
                custom,
            });
        }
    }
    Ok(out)
}

/// Signs the `releases` role with its online key: the previous entries plus `entries`. A name that
/// is already published with other bytes is refused: a published build never changes.
pub fn sign_releases(
    dir: &Path,
    key: &KeyFile,
    entries: &[Entry],
    now: Timestamp,
    days_: i64,
) -> Res<u64> {
    let (_, t) = latest_targets(dir)?;
    let deleg = delegations_of(&t)?;
    let (v, mut role) = match latest(dir, RELEASES)? {
        Some((v, b)) => {
            let s: Signed<Targets> = serde_json::from_slice(&b).map_err(e("releases.json"))?;
            (v, s.signed)
        }
        None => (0, Targets::new(SPEC.into(), nz(1), expiry(now, days_))),
    };
    role.delegations = None;
    for en in entries {
        let name = TargetName::new(&en.name).map_err(e(&en.name))?;
        let tgt = en.target()?;
        if let Some(old) = role.targets.get(&name) {
            if old != &tgt {
                return Err(format!(
                    "{} is already published with other bytes; a published file never changes",
                    en.name
                ));
            }
        }
        role.targets.insert(name, tgt);
    }
    role.version = nz(v + 1);
    role.expires = expiry(now, days_);
    let sr = run(SignedRole::new(
        DelegatedTargets {
            name: RELEASES.into(),
            targets: role,
        },
        &KeyHolder::Delegations(deleg),
        &sources(std::slice::from_ref(key)),
        &SystemRandom::new(),
    ))?
    .map_err(e("sign releases"))?;
    write_file(dir, &format!("{}.{RELEASES}.json", v + 1), sr.buffer())?;
    Ok(v + 1)
}

fn metafile(version: u64, bytes: &[u8]) -> Metafile {
    Metafile {
        length: Some(bytes.len() as u64),
        hashes: Some(Hashes {
            sha256: digest(&SHA256, bytes).as_ref().to_vec().into(),
            _extra: HashMap::new(),
        }),
        version: nz(version),
        _extra: HashMap::new(),
    }
}

/// Snapshot: the newest `targets` and `releases` (version, length, hash). Online, daily or
/// weekly and with every change.
pub fn sign_snapshot(dir: &Path, key: &KeyFile, now: Timestamp, days_: i64) -> Res<u64> {
    let (_, root) = latest_root(dir)?;
    let v = latest(dir, "snapshot")?.map_or(0, |(v, _)| v);
    let mut s = Snapshot::new(SPEC.into(), nz(v + 1), expiry(now, days_));
    let (tv, tb) = need(latest(dir, "targets")?, "targets")?;
    s.meta.insert("targets.json".into(), metafile(tv, &tb));
    let (rv, rb) = need(latest(dir, RELEASES)?, "releases")?;
    s.meta.insert(format!("{RELEASES}.json"), metafile(rv, &rb));
    let sr = run(SignedRole::new(
        s,
        &KeyHolder::Root(root),
        &sources(std::slice::from_ref(key)),
        &SystemRandom::new(),
    ))?
    .map_err(e("sign snapshot"))?;
    write_file(dir, &format!("{}.snapshot.json", v + 1), sr.buffer())?;
    Ok(v + 1)
}

/// Timestamp: the newest snapshot. Written as `timestamp.json`, the one file that is replaced.
pub fn sign_timestamp(dir: &Path, key: &KeyFile, now: Timestamp, days_: i64) -> Res<u64> {
    let (_, root) = latest_root(dir)?;
    let prev: u64 = match std::fs::read(dir.join("timestamp.json")) {
        Ok(b) => serde_json::from_slice::<Signed<tough::schema::Timestamp>>(&b)
            .map_err(e("timestamp.json"))?
            .signed
            .version
            .get(),
        Err(_) => 0,
    };
    let mut t = tough::schema::Timestamp::new(SPEC.into(), nz(prev + 1), expiry(now, days_));
    let (sv, sb) = need(latest(dir, "snapshot")?, "snapshot")?;
    t.meta.insert("snapshot.json".into(), metafile(sv, &sb));
    let sr = run(SignedRole::new(
        t,
        &KeyHolder::Root(root),
        &sources(std::slice::from_ref(key)),
        &SystemRandom::new(),
    ))?
    .map_err(e("sign timestamp"))?;
    write_file(dir, "timestamp.json", sr.buffer())?;
    Ok(prev + 1)
}

/// Which keys a rotation replaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rotate {
    Root,
    Targets,
    Snapshot,
    Timestamp,
}

pub struct RotateSpec<'a> {
    pub dir: &'a Path,
    pub what: Rotate,
    /// The replacement key(s): for `Root` the whole new set (threshold `root_threshold`), else one.
    pub new_keys: Vec<KeyFile>,
    pub root_threshold: Option<u64>,
    /// The current root keys, enough to meet the current threshold.
    pub old_root_keys: Vec<KeyFile>,
    pub now: Timestamp,
}

/// A new root version naming the replacement keys, signed by the old root's threshold and by the
/// new root's own. Refused, and nothing written, if either threshold is not met. The retired key
/// signs nothing afterwards: it is no longer named by any role. Afterwards the roles whose key
/// changed must be re-signed with the new key (`sign targets|snapshot|timestamp`).
pub fn rotate(a: &RotateSpec) -> Res<u64> {
    let (v, old) = latest_root(a.dir)?;
    let mut new = old.clone();
    new.version = nz(v + 1);
    new.expires = expiry(a.now, ROOT_DAYS);
    if a.new_keys.is_empty() {
        return Err("name the replacement key".into());
    }
    let refs: Vec<&KeyFile> = a.new_keys.iter().collect();
    let (role, threshold) = match a.what {
        Rotate::Root => (
            RoleType::Root,
            a.root_threshold
                .unwrap_or_else(|| old.roles[&RoleType::Root].threshold.get()),
        ),
        Rotate::Targets => (RoleType::Targets, 1),
        Rotate::Snapshot => (RoleType::Snapshot, 1),
        Rotate::Timestamp => (RoleType::Timestamp, 1),
    };
    if a.what != Rotate::Root && a.new_keys.len() != 1 {
        return Err("a role other than root has one key".into());
    }
    if threshold as usize > a.new_keys.len() || threshold == 0 {
        return Err("the threshold is more than the keys given".into());
    }
    for k in &a.new_keys {
        new.keys.insert(k.id()?, k.public()?);
    }
    new.roles.insert(role, role_keys(&refs, threshold)?);
    // a key no role names is dropped
    let named: std::collections::HashSet<_> = new
        .roles
        .values()
        .flat_map(|r| r.keyids.iter().cloned())
        .collect();
    new.keys.retain(|id, _| named.contains(id));
    // the new root is signed by the old root's keys, and by its own
    let mut new_signers = a.old_root_keys.clone();
    if a.what == Rotate::Root {
        new_signers = a.new_keys.clone();
    }
    let signed = cross_sign(&new, &old, &new_signers, &a.old_root_keys)?;
    if valid_signatures(&signed, &old) < old.roles[&RoleType::Root].threshold.get() as usize {
        return Err("too few of the old root keys were given to meet the old threshold".into());
    }
    if valid_signatures(&signed, &new) < new.roles[&RoleType::Root].threshold.get() as usize {
        return Err("too few of the new root keys were given to meet the new threshold".into());
    }
    write_file(a.dir, &format!("{}.root.json", v + 1), signed.buffer())?;
    Ok(v + 1)
}

/// Replaces the `releases` delegation's key: a new targets version (offline key) naming it. The
/// next `sign releases` must use the new key.
pub fn rotate_releases(
    dir: &Path,
    targets_key: &KeyFile,
    new_releases_key: &KeyFile,
    now: Timestamp,
) -> Res<u64> {
    let (_, root) = latest_root(dir)?;
    let (v, mut t) = latest_targets(dir)?;
    let mut d = delegations_of(&t)?;
    d.keys.clear();
    d.keys
        .insert(new_releases_key.id()?, new_releases_key.public()?);
    for r in d.roles.iter_mut().filter(|r| r.name == RELEASES) {
        r.keyids = vec![new_releases_key.id()?];
    }
    t.delegations = Some(d);
    t.version = nz(v + 1);
    t.expires = expiry(now, TARGETS_DAYS);
    let st = run(SignedRole::new(
        t,
        &KeyHolder::Root(root),
        &sources(std::slice::from_ref(targets_key)),
        &SystemRandom::new(),
    ))?
    .map_err(e("sign targets"))?;
    write_file(dir, &format!("{}.targets.json", v + 1), st.buffer())?;
    Ok(v + 1)
}

/// The two lines `crates/cigi-source/src/tuf_floor.rs` carries, from the metadata just signed.
pub fn floor_lines(dir: &Path) -> Res<String> {
    let (s, _) = need(latest(dir, "snapshot")?, "snapshot")?;
    let (t, _) = need(latest(dir, "targets")?, "targets")?;
    Ok(format!(
        "pub const MIN_SNAPSHOT_VERSION: u64 = {s};\npub const MIN_TARGETS_VERSION: u64 = {t};\n"
    ))
}

/// Every file of a signed set, for the publishing job: root chain, delegations, snapshot, then
/// timestamp last (decision 0004, section 7).
pub fn publish_order(dir: &Path) -> Res<Vec<PathBuf>> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(e("metadata directory"))?
        .flatten()
        .map(|d| d.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    let rank = |p: &PathBuf| {
        let n = p.file_name().map(|n| n.to_string_lossy().to_string());
        match n.as_deref() {
            Some("timestamp.json") => 3,
            Some(n) if n.ends_with(".snapshot.json") => 2,
            _ => 1,
        }
    };
    v.sort_by_key(|p| (rank(p), p.clone()));
    Ok(v)
}
