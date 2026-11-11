// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! The attacks TUF stops, written against the installer's own `TufTrust` and the metadata
//! `cargo xtask tuf` writes (decision 0004). Every key is generated when a test runs, lives in a
//! temporary directory and is gone when the test ends; nothing here is a real key.
//!
//! Each attack is a refusal: `admit_manifest` or `admit_artifact` returns an error. Each is paired
//! with the control, so a refusal is known to be about the attack and not about the fixture.

use cig_compat::Manifest;
use cigi_source::{NoAttest, SeenState, Trust, TrustLevel, TufConfig, TufTrust};
use cigi_testkit::Fixture;
use jiff::Timestamp;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tough::schema::{Root, Signed};
use tough::FilesystemTransport;
use xtask::tuf::{self, days, Entry, KeyFile, Rotate, RotateSpec};

struct Keys {
    root: Vec<KeyFile>,
    targets: KeyFile,
    releases: KeyFile,
    snapshot: KeyFile,
    timestamp: KeyFile,
}

struct World {
    tmp: tempfile::TempDir,
    keys: Keys,
    meta: PathBuf,
    manifest: String,
    fx: Fixture,
}

fn gen(dir: &Path, name: &str) -> KeyFile {
    let p = dir.join(format!("{name}.key"));
    tuf::keygen(&p, false).expect("keygen");
    KeyFile::read(&p).expect("read key")
}

fn manifest_text() -> (Fixture, String) {
    let mut fx = Fixture::new();
    fx.product("cig", "1.0.0");
    fx.carton("0.1.0", &[("cig", "1.0.0")]);
    fx.publish("stable");
    let t = std::fs::read_to_string(fx.releases.join("manifest-stable.toml")).expect("manifest");
    (fx, t)
}

impl World {
    /// A repository at version 1 of every role, signed as of `now`.
    fn at(now: Timestamp) -> World {
        let tmp = tempfile::tempdir().expect("tempdir");
        let k = |n: &str| gen(tmp.path(), n);
        let keys = Keys {
            root: vec![k("root-a"), k("root-b"), k("root-c")],
            targets: k("targets"),
            releases: k("releases"),
            snapshot: k("snapshot"),
            timestamp: k("timestamp"),
        };
        let meta = tmp.path().join("meta");
        tuf::init(&tuf::Init {
            dir: &meta,
            root_keys: keys.root.clone(),
            root_threshold: 2,
            targets_key: keys.targets.clone(),
            releases_key: keys.releases.clone(),
            snapshot_key: keys.snapshot.clone(),
            timestamp_key: keys.timestamp.clone(),
            now,
        })
        .expect("init");
        let (fx, manifest) = manifest_text();
        let w = World {
            tmp,
            keys,
            meta,
            manifest,
            fx,
        };
        w.release(now);
        w
    }
    fn live() -> World {
        World::at(Timestamp::now())
    }
    /// Sign the manifest and its builds, then snapshot and timestamp.
    fn release(&self, now: Timestamp) {
        let entries = tuf::entries_for(&self.manifest).expect("entries");
        tuf::sign_releases(&self.meta, &self.keys.releases, &entries, now, 30).expect("releases");
        self.refresh(now);
    }
    fn refresh(&self, now: Timestamp) {
        tuf::sign_snapshot(&self.meta, &self.keys.snapshot, now, 30).expect("snapshot");
        tuf::sign_timestamp(&self.meta, &self.keys.timestamp, now, 7).expect("timestamp");
    }
    fn root1(&self) -> Vec<u8> {
        std::fs::read(self.meta.join("1.root.json")).expect("root")
    }
    fn client(&self, ds: &str) -> TufTrust {
        self.client_with(ds, self.root1(), 1, 1)
    }
    fn client_with(&self, ds: &str, root: Vec<u8>, min_snap: u64, min_targets: u64) -> TufTrust {
        TufTrust::new(TufConfig {
            root,
            metadata_url: url::Url::from_directory_path(&self.meta).expect("url"),
            datastore: self.tmp.path().join(ds),
            min_snapshot_version: min_snap,
            min_targets_version: min_targets,
        })
        .with_transport(FilesystemTransport)
        .with_attest(Arc::new(NoAttest))
        .with_pins(&[])
    }
    fn admit(&self, t: &TufTrust) -> Result<(), String> {
        t.admit_manifest(self.manifest.as_bytes(), None, None, &seen())
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    fn snapshot_dir(&self, name: &str) -> PathBuf {
        let to = self.tmp.path().join(name);
        copy_dir(&self.meta, &to);
        to
    }
    /// What a mirror does when it serves an older copy.
    fn serve_copy(&self, copy: &Path) {
        std::fs::remove_dir_all(&self.meta).expect("rm");
        copy_dir(copy, &self.meta);
    }
}

fn seen() -> SeenState {
    SeenState {
        channel: Some("stable".into()),
        ..SeenState::default()
    }
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("mkdir");
    for e in std::fs::read_dir(from).expect("read dir").flatten() {
        std::fs::copy(e.path(), to.join(e.file_name())).expect("copy");
    }
}

fn root_of(dir: &Path, v: u64) -> Root {
    let b = std::fs::read(dir.join(format!("{v}.root.json"))).expect("root file");
    serde_json::from_slice::<Signed<Root>>(&b)
        .expect("parse")
        .signed
}

fn refused(r: Result<(), String>) -> String {
    r.expect_err("must be refused")
}

// ---- the control -------------------------------------------------------------------------

#[test]
fn control_a_correctly_signed_set_admits_the_manifest_and_its_builds() {
    let w = World::live();
    let t = w.client("ds");
    let m = t
        .admit_manifest(w.manifest.as_bytes(), None, None, &seen())
        .expect("admitted");
    let manifest = Manifest::from_str(&w.manifest).expect("parse");
    let art = manifest
        .builds
        .values()
        .next()
        .expect("a build")
        .artifacts
        .values()
        .next()
        .expect("an artifact");
    let file = w.fx.releases.join(&art.asset);
    let v = t
        .admit_artifact(art, None, &file)
        .expect("artifact admitted");
    assert_eq!(v.level, TrustLevel::Signed);
    assert_eq!(v.sha256, art.sha256);
    assert!(m.notes().iter().any(|n| n.contains("signed metadata")));
}

#[test]
fn an_artifact_before_any_manifest_is_refused() {
    let w = World::live();
    let t = w.client("ds");
    let manifest = Manifest::from_str(&w.manifest).expect("parse");
    let art = manifest
        .builds
        .values()
        .next()
        .expect("b")
        .artifacts
        .values()
        .next()
        .expect("a");
    assert!(t
        .admit_artifact(art, None, &w.fx.releases.join(&art.asset))
        .is_err());
}

// ---- rollback ----------------------------------------------------------------------------

#[test]
fn rollback_valid_older_metadata_after_newer_was_seen_is_refused() {
    let w = World::live();
    let old = w.snapshot_dir("v1");
    w.refresh(Timestamp::now()); // v2 of snapshot and timestamp
    let t = w.client("ds");
    w.admit(&t).expect("v2 admitted");
    w.serve_copy(&old); // the mirror now serves the older, once-valid set
    refused(w.admit(&t));
}

#[test]
fn rollback_control_a_client_with_no_history_accepts_old_metadata_unless_the_floor_says_no() {
    let w = World::live();
    w.refresh(Timestamp::now());
    // a fresh machine, floor 1: nothing to compare with. (The datastore starts empty.)
    w.admit(&w.client("fresh")).expect("no history, floor met");
    // the compiled-in floor is what closes that gap
    let t = w.client_with("floored", w.root1(), 5, 1);
    assert!(refused(w.admit(&t)).contains("below"));
    let t = w.client_with("floored2", w.root1(), 1, 5);
    assert!(refused(w.admit(&t)).contains("below"));
}

#[test]
fn rollback_of_the_manifest_sequence_is_refused() {
    let w = World::live();
    let t = w.client("ds");
    let s = SeenState {
        sequence: Some(u64::MAX),
        ..seen()
    };
    assert!(t
        .admit_manifest(w.manifest.as_bytes(), None, None, &s)
        .is_err());
}

// ---- freeze ------------------------------------------------------------------------------

#[test]
fn freeze_an_expired_timestamp_is_refused() {
    let w = World::live();
    let long_ago = Timestamp::now() - days(10); // a 7-day timestamp, signed 10 days ago
    tuf::sign_timestamp(&w.meta, &w.keys.timestamp, long_ago, 7).expect("timestamp");
    refused(w.admit(&w.client("ds")));
}

#[test]
fn freeze_an_expired_snapshot_is_refused() {
    let w = World::live();
    let long_ago = Timestamp::now() - days(40);
    tuf::sign_snapshot(&w.meta, &w.keys.snapshot, long_ago, 30).expect("snapshot");
    tuf::sign_timestamp(&w.meta, &w.keys.timestamp, Timestamp::now(), 7).expect("timestamp");
    refused(w.admit(&w.client("ds")));
}

#[test]
fn freeze_an_expired_releases_role_is_refused() {
    let w = World::live();
    let long_ago = Timestamp::now() - days(40);
    tuf::sign_releases(&w.meta, &w.keys.releases, &[], long_ago, 30).expect("releases");
    w.refresh(Timestamp::now());
    refused(w.admit(&w.client("ds")));
}

#[test]
fn freeze_an_expired_targets_role_is_refused() {
    let w = World::live();
    let long_ago = Timestamp::now() - days(400);
    tuf::sign_targets(&w.meta, &w.keys.targets, long_ago, 365).expect("targets");
    w.refresh(Timestamp::now());
    refused(w.admit(&w.client("ds")));
}

#[test]
fn freeze_offline_waives_timestamp_and_snapshot_but_says_so_and_never_root_or_targets() {
    let w = World::live();
    let long_ago = Timestamp::now() - days(10);
    tuf::sign_timestamp(&w.meta, &w.keys.timestamp, long_ago, 7).expect("timestamp");
    let s = SeenState {
        offline: true,
        ..seen()
    };
    let a = w
        .client("ds")
        .admit_manifest(w.manifest.as_bytes(), None, None, &s)
        .expect("offline use of old metadata");
    assert!(a.notes().iter().any(|n| n.contains("expired timestamp")));
    // an expired targets role is not waived
    let w = World::live();
    tuf::sign_targets(&w.meta, &w.keys.targets, Timestamp::now() - days(400), 365).expect("t");
    w.refresh(Timestamp::now());
    assert!(w
        .client("ds")
        .admit_manifest(w.manifest.as_bytes(), None, None, &s)
        .is_err());
}

// ---- mix and match -----------------------------------------------------------------------

#[test]
fn mix_an_old_releases_file_under_a_new_snapshot_is_refused() {
    let w = World::live();
    let old = w.snapshot_dir("v1");
    // v2 of releases carries a new entry; the snapshot and timestamp that follow name its hash
    let extra = Entry::of_bytes("cig-9.9.9-any", b"another build");
    tuf::sign_releases(&w.meta, &w.keys.releases, &[extra], Timestamp::now(), 30).expect("v2");
    w.refresh(Timestamp::now());
    // the attacker swaps 2.releases.json for the once-valid first one
    std::fs::copy(old.join("1.releases.json"), w.meta.join("2.releases.json")).expect("swap");
    refused(w.admit(&w.client("ds")));
}

#[test]
fn mix_an_old_snapshot_under_a_new_timestamp_is_refused() {
    let w = World::live();
    let old = w.snapshot_dir("v1");
    w.refresh(Timestamp::now()); // snapshot v2, timestamp v2 names snapshot v2's hash
    std::fs::copy(old.join("1.snapshot.json"), w.meta.join("2.snapshot.json")).expect("swap");
    refused(w.admit(&w.client("ds")));
}

#[test]
fn mix_an_old_targets_under_a_new_snapshot_is_refused() {
    let w = World::live();
    let old = w.snapshot_dir("v1");
    tuf::sign_targets(&w.meta, &w.keys.targets, Timestamp::now(), 365).expect("targets v2");
    w.refresh(Timestamp::now());
    std::fs::copy(old.join("1.targets.json"), w.meta.join("2.targets.json")).expect("swap");
    refused(w.admit(&w.client("ds")));
}

// ---- bad mirror --------------------------------------------------------------------------

#[test]
fn bad_mirror_a_manifest_changed_by_one_byte_is_refused() {
    let w = World::live();
    let t = w.client("ds");
    let mut doc = w.manifest.clone().into_bytes();
    let last = doc.len() - 1;
    doc[last] ^= 1;
    assert!(t.admit_manifest(&doc, None, None, &seen()).is_err());
}

#[test]
fn bad_mirror_a_longer_or_truncated_manifest_is_refused() {
    let w = World::live();
    let t = w.client("ds");
    let longer = format!("{}\n# extra\n", w.manifest);
    assert!(t
        .admit_manifest(longer.as_bytes(), None, None, &seen())
        .is_err());
    let cut = &w.manifest.as_bytes()[..w.manifest.len() / 2];
    assert!(t.admit_manifest(cut, None, None, &seen()).is_err());
}

#[test]
fn bad_mirror_a_signed_file_served_as_another_is_refused() {
    let w = World::live();
    let t = w.client("ds");
    w.admit(&t).expect("admitted");
    let manifest = Manifest::from_str(&w.manifest).expect("parse");
    let art = manifest
        .builds
        .values()
        .next()
        .expect("b")
        .artifacts
        .values()
        .next()
        .expect("a");
    // the build's bytes replaced by the manifest's: right kind of file, wrong file
    let other = w.tmp.path().join("other");
    std::fs::write(&other, &w.manifest).expect("write");
    assert!(t.admit_artifact(art, None, &other).is_err());
    // same length, one byte changed
    let mut b = std::fs::read(w.fx.releases.join(&art.asset)).expect("asset");
    b[0] ^= 1;
    std::fs::write(&other, b).expect("write");
    assert!(t.admit_artifact(art, None, &other).is_err());
}

#[test]
fn bad_mirror_the_whole_repository_under_other_keys_is_refused() {
    let w = World::live();
    let evil = World::live(); // a complete, internally consistent set under unrelated keys
    let client = w.client("ds"); // built with the real root, before the mirror is replaced
    std::fs::remove_dir_all(&w.meta).expect("rm");
    copy_dir(&evil.meta, &w.meta);
    refused(w.admit(&client));
}

#[test]
fn bad_mirror_serving_nothing_is_refused() {
    let w = World::live();
    std::fs::remove_file(w.meta.join("timestamp.json")).expect("rm");
    refused(w.admit(&w.client("ds")));
}

// ---- keys and delegation -----------------------------------------------------------------

#[test]
fn stolen_snapshot_and_timestamp_keys_cannot_publish_a_build() {
    use tough::editor::signed::SignedRole;
    use tough::schema::{DelegatedTargets, KeyHolder, Targets};
    let w = World::live();
    let thief = gen(w.tmp.path(), "thief");
    // the attacker forges a releases role naming their own manifest, signed by their own key
    let mut deleg = tough::schema::Delegations::new();
    deleg
        .keys
        .insert(thief.id().unwrap(), thief.public().unwrap());
    deleg.roles.push(tough::schema::DelegatedRole {
        name: "releases".into(),
        keyids: vec![thief.id().unwrap()],
        threshold: std::num::NonZeroU64::new(1).unwrap(),
        paths: tough::schema::PathSet::Paths(vec![tough::schema::PathPattern::new("*").unwrap()]),
        terminating: true,
        targets: None,
    });
    let evil_manifest = b"format = 1\nchannel = \"stable\"\n";
    let mut role = Targets::new(
        "1.0.0".into(),
        std::num::NonZeroU64::new(2).unwrap(),
        Timestamp::now() + days(30),
    );
    role.delegations = None;
    let e = Entry::of_bytes("manifest-stable.toml", evil_manifest);
    let _ = e; // the entry goes in through tough's own type below
    let forged = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(SignedRole::new(
            DelegatedTargets {
                name: "releases".into(),
                targets: role,
            },
            &KeyHolder::Delegations(deleg),
            &[Box::new(KeyFileSource(thief.clone())) as Box<dyn tough::key_source::KeySource>],
            &aws_lc_rs::rand::SystemRandom::new(),
        ))
        .expect("forged role");
    std::fs::write(w.meta.join("2.releases.json"), forged.buffer()).expect("write");
    // with the stolen snapshot and timestamp keys the attacker re-signs the two online roles
    w.refresh(Timestamp::now());
    refused(w.admit(&w.client("ds")));
}

#[derive(Debug)]
struct KeyFileSource(KeyFile);
#[tough::async_trait]
impl tough::key_source::KeySource for KeyFileSource {
    async fn as_sign(
        &self,
    ) -> Result<Box<dyn tough::sign::Sign>, Box<dyn std::error::Error + Send + Sync + 'static>>
    {
        Ok(Box::new(tough::sign::parse_keypair(self.0.der())?))
    }
    async fn write(
        &self,
        _: &str,
        _: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
        Err("no".into())
    }
}

#[test]
fn a_releases_key_cannot_vouch_for_a_name_outside_its_paths() {
    let w = World::live();
    w.admit(&w.client("control")).expect("control");
    // the releases key signs a file the delegation does not cover; the client refuses the role
    let evil = Entry::of_bytes("evil-1.0.0-any", b"x");
    tuf::sign_releases(&w.meta, &w.keys.releases, &[evil], Timestamp::now(), 30).expect("signs");
    w.refresh(Timestamp::now());
    refused(w.admit(&w.client("ds")));
}

#[test]
fn signed_digest_answers_only_for_names_the_metadata_vouches_for() {
    let w = World::live();
    let t = w.client("ds");
    assert!(
        t.signed_digest("manifest-stable.toml").is_err(),
        "no manifest admitted yet"
    );
    w.admit(&t).expect("admitted");
    assert!(t.signed_digest("manifest-stable.toml").is_ok());
    assert!(t.signed_digest("install.sh").is_err());
}

#[test]
fn a_published_name_never_changes_its_bytes() {
    let w = World::live();
    let changed = Entry::of_bytes("manifest-stable.toml", b"something else");
    let e = tuf::sign_releases(&w.meta, &w.keys.releases, &[changed], Timestamp::now(), 30)
        .expect_err("refused");
    assert!(e.contains("never changes"));
}

// ---- threshold and rotation --------------------------------------------------------------

#[test]
fn a_threshold_not_met_is_refused_at_the_tool_and_at_the_client() {
    let w = World::live();
    let (a, b, c) = (&w.keys.root[0], &w.keys.root[1], &w.keys.root[2]);
    let nk = gen(w.tmp.path(), "new-ts");
    // the tool: one of two old keys
    let e = tuf::rotate(&RotateSpec {
        dir: &w.meta,
        what: Rotate::Timestamp,
        new_keys: vec![nk.clone()],
        root_threshold: None,
        old_root_keys: vec![a.clone()],
        now: Timestamp::now(),
    })
    .expect_err("one old key is not enough");
    assert!(e.contains("too few"));
    assert!(!w.meta.join("2.root.json").exists());
    // the client: a root 2 naming {d, e, f}, signed by all three new keys and by only one old key
    let (d, e2, f) = (
        gen(w.tmp.path(), "root-d"),
        gen(w.tmp.path(), "root-e"),
        gen(w.tmp.path(), "root-f"),
    );
    let old = root_of(&w.meta, 1);
    let mut new = old.clone();
    new.version = std::num::NonZeroU64::new(2).unwrap();
    for k in [&d, &e2, &f] {
        new.keys.insert(k.id().unwrap(), k.public().unwrap());
    }
    new.roles
        .get_mut(&tough::schema::RoleType::Root)
        .unwrap()
        .keyids = vec![d.id().unwrap(), e2.id().unwrap(), f.id().unwrap()];
    let signers = [d.clone(), e2.clone(), f.clone()];
    let _ = (b, c);
    let client = w.client("ds");
    let bad = tuf::cross_sign(&new, &old, &signers, std::slice::from_ref(a)).expect("forged");
    std::fs::write(w.meta.join("2.root.json"), bad.buffer()).expect("write");
    refused(w.admit(&client));
    // control: the same root with two old signatures is followed
    let good = tuf::cross_sign(&new, &old, &signers, &[a.clone(), b.clone()]).expect("good");
    std::fs::write(w.meta.join("2.root.json"), good.buffer()).expect("write");
    // (the online roles are still signed by the old targets/snapshot/timestamp keys, which root 2 keeps)
    w.admit(&w.client("ds2"))
        .expect("two old signatures meet the old threshold");
}

#[test]
fn a_new_root_signed_by_too_few_new_keys_is_refused_by_the_client() {
    let w = World::live();
    let (a, b, c) = (&w.keys.root[0], &w.keys.root[1], &w.keys.root[2]);
    let d = gen(w.tmp.path(), "root-d");
    let e = gen(w.tmp.path(), "root-e");
    // root 2 names {d, e} with threshold 2; the old keys sign it, only d signs as the new set
    let old = root_of(&w.meta, 1);
    let mut new = old.clone();
    new.version = std::num::NonZeroU64::new(2).unwrap();
    new.keys.insert(d.id().unwrap(), d.public().unwrap());
    new.keys.insert(e.id().unwrap(), e.public().unwrap());
    let rk = new.roles.get_mut(&tough::schema::RoleType::Root).unwrap();
    rk.keyids = vec![d.id().unwrap(), e.id().unwrap()];
    let bad = tuf::cross_sign(
        &new,
        &old,
        std::slice::from_ref(&d),
        &[a.clone(), b.clone()],
    )
    .expect("forged");
    let _ = c;
    std::fs::write(w.meta.join("2.root.json"), bad.buffer()).expect("write");
    refused(w.admit(&w.client("ds")));
}

#[test]
fn rotating_the_timestamp_key_is_followed_by_an_old_client_and_the_retired_key_is_dead() {
    let w = World::live();
    let old_client = w.client("ds");
    w.admit(&old_client).expect("before");
    let new_ts = gen(w.tmp.path(), "new-ts");
    let before = w.snapshot_dir("before-rotation");
    tuf::rotate(&RotateSpec {
        dir: &w.meta,
        what: Rotate::Timestamp,
        new_keys: vec![new_ts.clone()],
        root_threshold: None,
        old_root_keys: vec![w.keys.root[0].clone(), w.keys.root[1].clone()],
        now: Timestamp::now(),
    })
    .expect("rotate");
    // not yet re-signed: the old timestamp no longer verifies under root 2
    refused(w.admit(&w.client("fresh-before-resign")));
    tuf::sign_timestamp(&w.meta, &new_ts, Timestamp::now(), 7).expect("new timestamp");
    // a client that only ever shipped with root 1 follows the chain
    w.admit(&w.client("fresh"))
        .expect("fresh client follows root 2");
    w.admit(&old_client)
        .expect("the client that saw v1 follows too");
    // the retired key signs a newer timestamp (made where root 2 does not exist)
    let attic = before;
    tuf::sign_timestamp(&attic, &w.keys.timestamp, Timestamp::now(), 7).expect("attacker");
    std::fs::copy(attic.join("timestamp.json"), w.meta.join("timestamp.json")).expect("serve");
    refused(w.admit(&w.client("fresh2")));
}

#[test]
fn replacing_a_lost_root_key_needs_two_old_keys_and_is_followed() {
    let w = World::live();
    let d = gen(w.tmp.path(), "root-d");
    let (a, b) = (w.keys.root[0].clone(), w.keys.root[1].clone());
    // root c is lost: the new set is {a, b, d}
    tuf::rotate(&RotateSpec {
        dir: &w.meta,
        what: Rotate::Root,
        new_keys: vec![a.clone(), b.clone(), d.clone()],
        root_threshold: Some(2),
        old_root_keys: vec![a.clone(), b.clone()],
        now: Timestamp::now(),
    })
    .expect("rotate");
    w.admit(&w.client("fresh"))
        .expect("an installer built with root 1 follows root 2");
    let r2 = root_of(&w.meta, 2);
    assert!(
        !r2.keys.contains_key(&w.keys.root[2].id().unwrap()),
        "the lost key is retired"
    );
}

#[test]
fn rotating_the_releases_delegation_key_retires_the_old_releases_key() {
    let w = World::live();
    let new_rel = gen(w.tmp.path(), "new-releases");
    tuf::rotate_releases(&w.meta, &w.keys.targets, &new_rel, Timestamp::now()).expect("rotate");
    // the old key can no longer sign a release
    let again = Entry::of_bytes("cig-9.9.9-any", b"x");
    assert!(tuf::sign_releases(
        &w.meta,
        &w.keys.releases,
        std::slice::from_ref(&again),
        Timestamp::now(),
        30
    )
    .is_err());
    tuf::sign_releases(&w.meta, &new_rel, &[again], Timestamp::now(), 30).expect("new key signs");
    w.refresh(Timestamp::now());
    w.admit(&w.client("fresh"))
        .expect("admitted under the new delegation");
}

// ---- key files and the pieces around them ------------------------------------------------

#[test]
fn a_release_needs_only_the_online_keys() {
    // After init, a second release is signed with releases, snapshot and timestamp keys alone:
    // the root and targets keys are not read.
    let w = World::live();
    let dir = w.tmp.path().join("online-only");
    let _ = dir;
    let extra = Entry::of_bytes("cig-1.0.1-any", b"next build");
    tuf::sign_releases(&w.meta, &w.keys.releases, &[extra], Timestamp::now(), 30).expect("v2");
    w.refresh(Timestamp::now());
    let t = w.client("fresh");
    w.admit(&t).expect("admitted");
    assert!(t.signed_digest("cig-1.0.1-any").is_ok());
}

#[test]
fn key_files_may_be_der_or_base64_and_never_overwrite() {
    let d = tempfile::tempdir().expect("tmp");
    let der = d.path().join("k.der");
    let b64 = d.path().join("k.b64");
    let id1 = tuf::keygen(&der, false).expect("der");
    let id2 = tuf::keygen(&b64, true).expect("b64");
    assert_ne!(id1, id2);
    assert!(KeyFile::read(&b64).is_ok());
    assert!(
        tuf::keygen(&der, false).is_err(),
        "an existing key is never overwritten"
    );
    let secret = std::fs::read_to_string(&b64).expect("text");
    assert!(secret.trim().is_ascii());
}

#[test]
fn the_tool_refuses_a_key_path_inside_the_repository() {
    let inside = Path::new(env!("CARGO_MANIFEST_DIR")).join("never-written.key");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["tuf", "keygen"])
        .arg(&inside)
        .output()
        .expect("run");
    assert!(!out.status.success());
    assert!(!inside.exists());
    assert!(String::from_utf8_lossy(&out.stderr).contains("inside the repository"));
}

#[test]
fn the_floor_lines_follow_the_signed_versions() {
    let w = World::live();
    w.refresh(Timestamp::now());
    let l = tuf::floor_lines(&w.meta).expect("floor");
    assert!(l.contains("MIN_SNAPSHOT_VERSION: u64 = 2"));
    assert!(l.contains("MIN_TARGETS_VERSION: u64 = 1"));
}

#[test]
fn entries_carry_the_manifest_and_every_build() {
    let (_fx, text) = manifest_text();
    let es = tuf::entries_for(&text).expect("entries");
    assert!(es.iter().any(|e| e.name == "manifest-stable.toml"));
    assert!(es.len() >= 2);
}
