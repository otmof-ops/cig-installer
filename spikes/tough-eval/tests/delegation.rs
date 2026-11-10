// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! Delegations per product: each product's release pipeline holds one key, and the top-level
//! targets role (signed offline) says which file names that key may sign. Here `cig` may sign
//! `cig-*` and `cigo` may sign `cigo-*`.

use jiff::Timestamp;
use tough::editor::RepositoryEditor;
use tough::schema::{PathPattern, PathSet};
use tough::ExpirationEnforcement::Safe;
use tough_eval::fixture::*;

async fn editor_with_delegations(
    w: &World,
    cig_key: &TestKey,
    cigo_key: &TestKey,
    cig_path: &str,
) -> RepositoryEditor {
    let m = w.path("m");
    w.root_signed.write(m.join("metadata"), true).await.unwrap();
    let mut ed = RepositoryEditor::new(m.join("metadata/1.root.json"))
        .await
        .unwrap();
    let exp = Timestamp::now() + days(30);
    ed.targets_version(nz(1))
        .unwrap()
        .targets_expires(exp)
        .unwrap()
        .snapshot_version(nz(1))
        .snapshot_expires(Timestamp::now() + days(7))
        .timestamp_version(nz(1))
        .timestamp_expires(Timestamp::now() + days(1));
    ed.delegate_role(
        "cig",
        &sources(&[cig_key]),
        PathSet::Paths(vec![PathPattern::new(cig_path).unwrap()]),
        true,
        nz(1),
        exp,
        nz(1),
    )
    .await
    .unwrap()
    .delegate_role(
        "cigo",
        &sources(&[cigo_key]),
        PathSet::Paths(vec![PathPattern::new("cigo-*").unwrap()]),
        true,
        nz(1),
        exp,
        nz(1),
    )
    .await
    .unwrap();
    ed
}

async fn add_to_role(
    ed: &mut RepositoryEditor,
    role: &str,
    name: &str,
    bytes: &[u8],
    key: &TestKey,
    top: &[Box<dyn tough::key_source::KeySource>],
) {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join(name);
    std::fs::write(&p, bytes).unwrap();
    ed.sign_targets_editor(top).await.unwrap();
    ed.change_delegated_targets(role).unwrap();
    ed.add_target_path(&p).await.unwrap();
    ed.targets_version(nz(1))
        .unwrap()
        .targets_expires(Timestamp::now() + days(30))
        .unwrap();
    ed.sign_targets_editor(&sources(&[key])).await.unwrap();
    ed.change_delegated_targets("targets").unwrap();
    ed.targets_version(nz(1))
        .unwrap()
        .targets_expires(Timestamp::now() + days(30))
        .unwrap();
}

#[tokio::test]
async fn each_product_key_signs_only_its_own_files() {
    let w = World::new().await;
    let cig = TestKey::generate(w.dir.path(), "cig-pipeline");
    let cigo = TestKey::generate(w.dir.path(), "cigo-pipeline");
    let top = sources(&[&w.keys.targets]);

    let mut ed = editor_with_delegations(&w, &cig, &cigo, "cig-*").await;
    add_to_role(
        &mut ed,
        "cig",
        "cig-1.20.1-linux-x86_64",
        b"cig build",
        &cig,
        &top,
    )
    .await;
    add_to_role(
        &mut ed,
        "cigo",
        "cigo-1.17.0-linux-x86_64",
        b"cigo build",
        &cigo,
        &top,
    )
    .await;
    let signed = ed.sign(&w.keys.all()).await.unwrap();
    let m = w.path("m");
    signed.write(m.join("metadata")).await.unwrap();
    // Targets are copied from per-role source dirs in a real pipeline; here the bytes are written
    // under their hash-prefixed names directly.
    std::fs::create_dir_all(m.join("targets")).unwrap();
    for (name, bytes) in [
        ("cig-1.20.1-linux-x86_64", &b"cig build"[..]),
        ("cigo-1.17.0-linux-x86_64", &b"cigo build"[..]),
    ] {
        let sha =
            hex::encode(aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, bytes).as_ref());
        std::fs::write(m.join("targets").join(format!("{sha}.{name}")), bytes).unwrap();
    }

    let p = Published {
        meta: m.join("metadata"),
        targets: m.join("targets"),
    };
    let repo = load(&w.trusted_root(), &p, &w.path("ds"), Safe)
        .await
        .unwrap();
    assert!(repo.delegated_role("cig").is_some() && repo.delegated_role("cigo").is_some());
    assert_eq!(
        fetch(&repo, "cig-1.20.1-linux-x86_64")
            .await
            .unwrap()
            .as_deref(),
        Some(&b"cig build"[..])
    );
    assert_eq!(
        fetch(&repo, "cigo-1.17.0-linux-x86_64")
            .await
            .unwrap()
            .as_deref(),
        Some(&b"cigo build"[..])
    );
    assert_eq!(repo.all_targets().count(), 2);
}

/// A compromised cig pipeline that signs a file in cigo's namespace is refused when the repository
/// is built, so it cannot reach a snapshot. (Refusal on the client side needs metadata that the
/// editor will not produce; the client-side path check is covered by `tough`'s own suite.)
#[tokio::test]
async fn a_pipeline_key_cannot_sign_another_products_file() {
    let w = World::new().await;
    let cig = TestKey::generate(w.dir.path(), "cig-pipeline");
    let cigo = TestKey::generate(w.dir.path(), "cigo-pipeline");
    let top = sources(&[&w.keys.targets]);

    // cig's delegation is as it should be (`cig-*`); its pipeline signs a `cigo-` file anyway.
    let mut ed = editor_with_delegations(&w, &cig, &cigo, "cig-*").await;
    add_to_role(
        &mut ed,
        "cig",
        "cigo-9.9.9-linux-x86_64",
        b"planted",
        &cig,
        &top,
    )
    .await;
    let err = ed.sign(&w.keys.all()).await.unwrap_err();
    assert!(
        matches!(err, tough::error::Error::InvalidPath { .. }),
        "expected the path check to refuse it, got {err}"
    );
}
