// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! The four attacks issue #9 names, each refused, plus the controls that show each test can pass
//! when nothing is wrong. A test that only ever fails proves nothing.

use jiff::Timestamp;
use tough::error::Error;
use tough::ExpirationEnforcement::{Safe, Unsafe};
use tough_eval::fixture::*;

/// `tough` reports a digest or length failure that happens while a file streams as a `Transport`
/// error whose cause carries the text, so those are matched on the message. The installer must
/// treat every `Err` from `tough` as a refusal and never branch on the variant (decision 0004,
/// section 8).
fn says(err: &Error, what: &str) -> bool {
    err.to_string().contains(what)
}

fn old_files() -> Vec<File> {
    vec![File::new(
        "cig-1.20.0-linux-x86_64",
        b"old build, has a known hole",
    )]
}
fn new_files() -> Vec<File> {
    vec![
        File::new("cig-1.20.0-linux-x86_64", b"old build, has a known hole"),
        File::new("cig-1.20.1-linux-x86_64", b"new build, hole fixed"),
    ]
}

#[tokio::test]
async fn control_an_honest_mirror_is_accepted_and_its_target_reads() {
    let w = World::new().await;
    let m = w.publish("m", &new_files(), &Release::at(2)).await;
    let repo = load(&w.trusted_root(), &m, &w.path("ds"), Safe)
        .await
        .unwrap();
    let got = fetch(&repo, "cig-1.20.1-linux-x86_64").await.unwrap();
    assert_eq!(got.as_deref(), Some(&b"new build, hole fixed"[..]));
    // A name the signed metadata does not list is "not found", never a fallback to the mirror.
    assert!(fetch(&repo, "cig-9.9.9-linux-x86_64")
        .await
        .unwrap()
        .is_none());
}

/// ROLLBACK. Metadata that was valid once and is still unexpired is replayed after the client has
/// seen newer metadata. The datastore is what remembers.
#[tokio::test]
async fn rollback_to_older_signed_metadata_is_refused() {
    let w = World::new().await;
    let v1 = w.publish("v1", &old_files(), &Release::at(1)).await;
    let v2 = w.publish("v2", &new_files(), &Release::at(2)).await;
    let ds = w.path("ds");

    load(&w.trusted_root(), &v2, &ds, Safe).await.unwrap();
    let err = load(&w.trusted_root(), &v1, &ds, Safe).await.unwrap_err();
    assert!(
        matches!(err, Error::OlderMetadata { .. }),
        "expected OlderMetadata, got {err}"
    );
}

/// The limit of rollback protection: it lasts only as long as the datastore. A machine that has
/// never seen v2 accepts v1, which is correctly signed and unexpired. This is why the installer
/// keeps its datastore under the home it owns and why the compiled-in floor (decision 0004,
/// section 5) exists: first use is the only moment a mirror can pick the version.
#[tokio::test]
async fn control_a_client_with_no_history_accepts_old_unexpired_metadata() {
    let w = World::new().await;
    let v1 = w.publish("v1", &old_files(), &Release::at(1)).await;
    let repo = load(&w.trusted_root(), &v1, &w.path("fresh"), Safe)
        .await
        .unwrap();
    assert_eq!(u64::from(repo.timestamp().signed.version), 1);
}

/// FREEZE. A mirror serves a timestamp that was valid and has since expired, to keep the client on
/// the version it already has. The signature is good; only the date is bad.
#[tokio::test]
async fn freeze_with_expired_timestamp_is_refused() {
    let w = World::new().await;
    let mut rel = Release::at(1);
    rel.timestamp_expires = Timestamp::now() - days(1);
    let m = w.publish("m", &old_files(), &rel).await;
    let err = load(&w.trusted_root(), &m, &w.path("ds"), Safe)
        .await
        .unwrap_err();
    assert!(
        matches!(err, Error::ExpiredMetadata { role, .. } if role == tough::schema::RoleType::Timestamp),
        "expected an expired timestamp, got {err}"
    );
}

#[tokio::test]
async fn freeze_with_expired_snapshot_or_targets_is_refused_too() {
    let w = World::new().await;
    let mut rel = Release::at(1);
    rel.snapshot_expires = Timestamp::now() - days(1);
    let m = w.publish("s", &old_files(), &rel).await;
    let err = load(&w.trusted_root(), &m, &w.path("ds1"), Safe)
        .await
        .unwrap_err();
    assert!(
        matches!(err, Error::ExpiredMetadata { .. }),
        "snapshot: {err}"
    );

    let mut rel = Release::at(1);
    rel.targets_expires = Timestamp::now() - days(1);
    let m = w.publish("t", &old_files(), &rel).await;
    let err = load(&w.trusted_root(), &m, &w.path("ds2"), Safe)
        .await
        .unwrap_err();
    assert!(
        matches!(err, Error::ExpiredMetadata { .. }),
        "targets: {err}"
    );
}

/// The one switch that turns the freeze defense off. The installer's `--offline` is the only
/// caller that may pass it, and it says so out loud (decision 0004, section 6).
#[tokio::test]
async fn control_unsafe_enforcement_is_what_lets_an_expired_timestamp_through() {
    let w = World::new().await;
    let mut rel = Release::at(1);
    rel.timestamp_expires = Timestamp::now() - days(1);
    let m = w.publish("m", &old_files(), &rel).await;
    assert!(load(&w.trusted_root(), &m, &w.path("ds"), Unsafe)
        .await
        .is_ok());
}

/// MIX AND MATCH, targets. The mirror serves the newest timestamp and snapshot, and an old but
/// validly signed targets file under the new name (to hide the fixed build, or to keep the old
/// list of acceptable files).
#[tokio::test]
async fn mix_and_match_old_targets_under_new_snapshot_is_refused() {
    let w = World::new().await;
    let v1 = w.publish("v1", &old_files(), &Release::at(1)).await;
    let v2 = w.publish("v2", &new_files(), &Release::at(2)).await;
    let mixed = copy_published(&v2, &w.path("mixed"));
    std::fs::copy(
        v1.meta.join("1.targets.json"),
        mixed.meta.join("2.targets.json"),
    )
    .unwrap();

    let err = load(&w.trusted_root(), &mixed, &w.path("ds"), Safe)
        .await
        .unwrap_err();
    assert!(
        says(&err, "Hash mismatch") || matches!(err, Error::VersionMismatch { .. }),
        "expected the snapshot to pin the targets file, got {err}"
    );
}

/// MIX AND MATCH, snapshot. The mirror serves the newest timestamp with an older snapshot.
#[tokio::test]
async fn mix_and_match_old_snapshot_under_new_timestamp_is_refused() {
    let w = World::new().await;
    let v1 = w.publish("v1", &old_files(), &Release::at(1)).await;
    let v2 = w.publish("v2", &new_files(), &Release::at(2)).await;
    let mixed = copy_published(&v2, &w.path("mixed"));
    std::fs::copy(
        v1.meta.join("1.snapshot.json"),
        mixed.meta.join("2.snapshot.json"),
    )
    .unwrap();

    let err = load(&w.trusted_root(), &mixed, &w.path("ds"), Safe)
        .await
        .unwrap_err();
    assert!(
        says(&err, "Hash mismatch")
            || matches!(
                err,
                Error::VersionMismatch { .. } | Error::OlderMetadata { .. }
            ),
        "expected the timestamp to pin the snapshot, got {err}"
    );
}

fn target_file(m: &Published, name: &str) -> std::path::PathBuf {
    std::fs::read_dir(&m.targets)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.file_name().unwrap().to_string_lossy().ends_with(name))
        .expect("a hash-prefixed target file")
}

/// BAD MIRROR. Metadata is honest; the bytes of a target are not.
#[tokio::test]
async fn bad_mirror_with_tampered_target_is_refused() {
    let w = World::new().await;
    let m = w.publish("m", &new_files(), &Release::at(2)).await;
    let name = "cig-1.20.1-linux-x86_64";

    // Same length, one byte changed.
    let f = target_file(&m, name);
    let mut b = std::fs::read(&f).unwrap();
    b[0] ^= 1;
    std::fs::write(&f, &b).unwrap();
    let repo = load(&w.trusted_root(), &m, &w.path("ds1"), Safe)
        .await
        .unwrap();
    let err = fetch(&repo, name).await.unwrap_err();
    assert!(says(&err, "Hash mismatch"), "same length: {err}");

    // Longer: a payload appended to a real build.
    b.extend_from_slice(b"; and a payload");
    std::fs::write(&f, &b).unwrap();
    let repo = load(&w.trusted_root(), &m, &w.path("ds2"), Safe)
        .await
        .unwrap();
    assert!(
        fetch(&repo, name).await.is_err(),
        "longer file was accepted"
    );

    // Shorter: truncated.
    std::fs::write(&f, &b[..4]).unwrap();
    let repo = load(&w.trusted_root(), &m, &w.path("ds3"), Safe)
        .await
        .unwrap();
    assert!(
        fetch(&repo, name).await.is_err(),
        "truncated file was accepted"
    );
}

/// BAD MIRROR, swapped file. A real, signed target served in place of another real one.
#[tokio::test]
async fn bad_mirror_serving_one_signed_target_as_another_is_refused() {
    let w = World::new().await;
    let m = w.publish("m", &new_files(), &Release::at(2)).await;
    let old = target_file(&m, "cig-1.20.0-linux-x86_64");
    let new = target_file(&m, "cig-1.20.1-linux-x86_64");
    std::fs::copy(&old, &new).unwrap();
    let repo = load(&w.trusted_root(), &m, &w.path("ds"), Safe)
        .await
        .unwrap();
    let err = fetch(&repo, "cig-1.20.1-linux-x86_64").await.unwrap_err();
    assert!(
        says(&err, "Hash mismatch") || says(&err, "Maximum size"),
        "{err}"
    );
}

/// A WHOLE REPOSITORY THE ATTACKER MADE, with their own keys, served where ours should be.
#[tokio::test]
async fn bad_mirror_serving_a_repository_signed_by_other_keys_is_refused() {
    let honest = World::new().await;
    let rogue = World::new().await;
    let m = rogue.publish("m", &new_files(), &Release::at(9)).await;
    let err = load(&honest.trusted_root(), &m, &honest.path("ds"), Safe)
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            Error::VerifyMetadata { .. } | Error::VerifyTrustedMetadata { .. }
        ),
        "{err}"
    );
}

/// The point of separate roles: stealing the online timestamp key does not let an attacker sign a
/// snapshot or targets. They can sign a fresh timestamp (that is the freeze and replay power the
/// short expiry bounds), and nothing the timestamp points to will verify.
#[tokio::test]
async fn a_stolen_timestamp_key_alone_cannot_publish_anything() {
    let honest = World::new().await;
    let atk_dir = tempfile::tempdir().unwrap();
    let own = KeySet::generate(atk_dir.path(), "atk");
    let stolen = &honest.keys.timestamp;
    // The attacker's root names the stolen key for every role it needs to sign, because it is the
    // only honest-root key they hold.
    let root = build_root(
        1,
        Timestamp::now() + days(30),
        &own.root.iter().collect::<Vec<_>>(),
        2,
        stolen,
        stolen,
        stolen,
    );
    let signed = sign_root(&root, &root, &[&own.root[0], &own.root[1]]).await;
    let mut keys = own.all();
    keys.push(stolen.source());
    let m = publish(
        &signed,
        &keys,
        &new_files(),
        &Release::at(50),
        atk_dir.path(),
    )
    .await;

    let err = load(&honest.trusted_root(), &m, &honest.path("ds"), Safe)
        .await
        .unwrap_err();
    assert!(
        matches!(err, Error::VerifyMetadata { role, .. } if role != tough::schema::RoleType::Timestamp),
        "expected the snapshot or targets to fail, got {err}"
    );
}

/// What the installer takes from signed targets metadata without downloading the target: the
/// length, the SHA-256, and the signed `custom` field, where a build's attestation reference goes.
#[tokio::test]
async fn signed_targets_carry_length_digest_and_attestation_reference() {
    let w = World::new().await;
    let f = File::new("cig-1.20.1-linux-x86_64", b"new build, hole fixed").with_custom(
        serde_json::json!({"attestation": {"repo": "otmof-ops/cig-script", "subject": "cig-1.20.1-linux-x86_64"}}),
    );
    let m = w.publish("m", &[f], &Release::at(1)).await;
    let repo = load(&w.trusted_root(), &m, &w.path("ds"), Safe)
        .await
        .unwrap();
    let (name, t) = repo.all_targets().next().unwrap();
    assert_eq!(name.raw(), "cig-1.20.1-linux-x86_64");
    assert_eq!(t.length, 21);
    assert_eq!(t.hashes.sha256.len(), 32);
    assert_eq!(t.custom["attestation"]["repo"], "otmof-ops/cig-script");
}
