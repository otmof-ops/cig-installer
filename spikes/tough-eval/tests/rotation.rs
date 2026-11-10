// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! The rotation rehearsal issue #9 asks for, run against `tough` with throwaway keys: a routine
//! rotation of the online timestamp key, a root key lost and replaced, and the refusals that make
//! a rotation safe (a root change signed by too few old keys, or by too few new ones).

use jiff::Timestamp;
use tough::error::Error;
use tough::schema::RoleType;
use tough::ExpirationEnforcement::Safe;
use tough_eval::fixture::*;

fn files() -> Vec<File> {
    vec![File::new("cigo-1.17.0-linux-x86_64", b"cigo build")]
}

/// Root v2: the timestamp key replaced (routine), root key C lost and D added (recovery). Signed by
/// two of the old root keys (A, B) and two of the new (A, D), so both thresholds hold.
#[tokio::test]
async fn rotating_the_timestamp_key_and_a_lost_root_key_is_followed_by_an_old_client() {
    let w = World::new().await;
    let new_ts = TestKey::generate(w.dir.path(), "ts2");
    let root_d = TestKey::generate(w.dir.path(), "root-d");
    let (a, b) = (&w.keys.root[0], &w.keys.root[1]);
    let v2 = build_root(
        2,
        Timestamp::now() + days(365),
        &[a, b, &root_d],
        2,
        &w.keys.targets,
        &w.keys.snapshot,
        &new_ts,
    );
    let v2_signed = rotate_root(&v2, &[a, &root_d], &w.root, &[a, b]).await;

    // Publish under the new root with the new timestamp key.
    let mut keys = w.keys.all();
    keys.push(new_ts.source());
    keys.push(root_d.source());
    let m = publish(&v2_signed, &keys, &files(), &Release::at(1), &w.path("m")).await;
    // The mirror also carries the first root, as a real one does, so the chain is walkable.
    std::fs::write(m.meta.join("1.root.json"), w.trusted_root()).unwrap();

    // A client that shipped with root v1 and has never seen v2 walks the chain.
    let repo = load(&w.trusted_root(), &m, &w.path("ds"), Safe)
        .await
        .unwrap();
    assert_eq!(u64::from(repo.root().signed.version), 2);
    assert_eq!(
        fetch(&repo, "cigo-1.17.0-linux-x86_64")
            .await
            .unwrap()
            .as_deref(),
        Some(&b"cigo build"[..])
    );
}

#[tokio::test]
async fn a_root_change_signed_by_too_few_old_keys_is_refused() {
    let w = World::new().await;
    let root_d = TestKey::generate(w.dir.path(), "root-d");
    let a = &w.keys.root[0];
    let v2 = build_root(
        2,
        Timestamp::now() + days(365),
        &[a, &w.keys.root[1], &root_d],
        2,
        &w.keys.targets,
        &w.keys.snapshot,
        &w.keys.timestamp,
    );
    // Two valid new signatures (A, D) but only one old one (A): an attacker holding one stolen
    // root key and one key they made themselves.
    let v2_signed = rotate_root(&v2, &[a, &root_d], &w.root, &[a]).await;
    let mut keys = w.keys.all();
    keys.push(root_d.source());
    let m = publish(&v2_signed, &keys, &files(), &Release::at(1), &w.path("m")).await;
    std::fs::write(m.meta.join("1.root.json"), w.trusted_root()).unwrap();

    let err = load(&w.trusted_root(), &m, &w.path("ds"), Safe)
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            Error::VerifyMetadata {
                role: RoleType::Root,
                ..
            }
        ),
        "expected root v2 to fail verification, got {err}"
    );
}

#[tokio::test]
async fn a_root_change_signed_by_too_few_new_keys_is_refused() {
    let w = World::new().await;
    // A full replacement of the root keys: three new keys, threshold 2.
    let n: Vec<TestKey> = (0..3)
        .map(|i| TestKey::generate(w.dir.path(), &format!("new-root-{i}")))
        .collect();
    let v2 = build_root(
        2,
        Timestamp::now() + days(365),
        &[&n[0], &n[1], &n[2]],
        2,
        &w.keys.targets,
        &w.keys.snapshot,
        &w.keys.timestamp,
    );
    // The old threshold is met (A, B) and the new one is not (no new key signed): a compromise of
    // the old keys that tries to hand the root to keys it does not control would still need them.
    let v2_signed = rotate_root(&v2, &[], &w.root, &[&w.keys.root[0], &w.keys.root[1]]).await;
    let mut keys = w.keys.all();
    keys.extend(n.iter().map(TestKey::source));
    let m = publish(&v2_signed, &keys, &files(), &Release::at(1), &w.path("m")).await;
    std::fs::write(m.meta.join("1.root.json"), w.trusted_root()).unwrap();

    let err = load(&w.trusted_root(), &m, &w.path("ds"), Safe)
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            Error::VerifyMetadata {
                role: RoleType::Root,
                ..
            }
        ),
        "expected root v2 to fail verification, got {err}"
    );
}

/// After rotation the retired timestamp key is worthless: metadata it signs is refused by a client
/// that has followed root v2, even though the same key was valid under root v1.
#[tokio::test]
async fn a_retired_timestamp_key_is_refused_after_rotation() {
    let w = World::new().await;
    let new_ts = TestKey::generate(w.dir.path(), "ts2");
    let v2 = build_root(
        2,
        Timestamp::now() + days(365),
        &w.keys.root.iter().collect::<Vec<_>>(),
        2,
        &w.keys.targets,
        &w.keys.snapshot,
        &new_ts,
    );
    let v2_signed = rotate_root(
        &v2,
        &[&w.keys.root[0], &w.keys.root[1]],
        &w.root,
        &[&w.keys.root[0], &w.keys.root[1]],
    )
    .await;

    // The attacker holds the OLD timestamp key and root v1's view of the world: metadata signed
    // under v1, served next to the genuine 2.root.json.
    let stale = w.publish("stale", &files(), &Release::at(5)).await;
    std::fs::write(stale.meta.join("2.root.json"), v2_signed.buffer()).unwrap();

    let err = load(&w.trusted_root(), &stale, &w.path("ds"), Safe)
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            Error::VerifyMetadata {
                role: RoleType::Timestamp,
                ..
            }
        ),
        "expected the old timestamp key to be refused, got {err}"
    );
}
