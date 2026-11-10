// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! Bundles (#16): the file format, what unpacking refuses, and the trust rule: a bundle is one
//! source, so it is `pinned` or refused, exactly as a mirror directory is.

mod common;
use cig_compat::Target;
use cigi_source::{
    get_artifact, get_manifest, unpack_bundle, write_bundle, BundleError, BundleFile, BundleSource,
    DigestTrust, MirrorSource, NoProgress, Source, TrustLevel,
};
use cigi_testkit::Fixture;
use common::*;
use std::path::Path;
use std::sync::Arc;

fn fixture() -> Fixture {
    let mut fx = Fixture::new();
    fx.carton("0.1.0", &[("cig", "1.0.0")]);
    fx.publish("stable");
    fx
}

/// Everything in `releases/` as a bundle.
fn bundle_of(fx: &Fixture, out: &Path) {
    let files: Vec<BundleFile> = std::fs::read_dir(&fx.releases)
        .unwrap()
        .flatten()
        .map(|e| {
            BundleFile::bytes(
                e.file_name().to_string_lossy().into_owned(),
                std::fs::read(e.path()).unwrap(),
            )
        })
        .collect();
    write_bundle(
        out,
        &[("channel", "stable".into()), ("carton", "0.1.0".into())],
        files,
    )
    .unwrap();
}

fn unpacked(fx: &Fixture, tar: &Path) -> (tempfile::TempDir, BundleSource) {
    let dir = tempfile::tempdir().unwrap();
    unpack_bundle(tar, dir.path()).unwrap();
    let src = BundleSource::new(dir.path(), "x.tar").unwrap();
    let _ = fx;
    (dir, src)
}

fn pin_table(fx: &Fixture) -> &'static [(&'static str, &'static str)] {
    let m = manifest_of(fx, "stable");
    let art = m.artifact("cig", &v("1.0.0"), &Target::host()).unwrap();
    let key = format!("cig-1.0.0-{}", Target::host());
    Box::leak(
        vec![(
            Box::leak(key.into_boxed_str()) as &'static str,
            Box::leak(art.sha256.clone().into_boxed_str()) as &'static str,
        )]
        .into_boxed_slice(),
    )
}

#[test]
fn a_bundle_round_trips_and_names_itself() {
    let fx = fixture();
    let tar = fx.tmp.path().join("b.tar");
    bundle_of(&fx, &tar);
    let dir = tempfile::tempdir().unwrap();
    let un = unpack_bundle(&tar, dir.path()).unwrap();
    assert_eq!(un.channels(), vec!["stable"]);
    assert_eq!(un.index["format"], 1);
    assert_eq!(un.index["signed"], false);
    assert_eq!(un.index["carton"], "0.1.0");
    for (name, size) in &un.files {
        assert_eq!(
            std::fs::read(dir.path().join(name)).unwrap(),
            std::fs::read(fx.releases.join(name)).unwrap(),
            "{name}"
        );
        assert_eq!(
            std::fs::metadata(dir.path().join(name)).unwrap().len(),
            *size
        );
    }
    let (_keep, src) = unpacked(&fx, &tar);
    assert_eq!(src.name(), "bundle:x.tar");
    assert!(!src.independent());
}

/// The same bytes every time: sorted, mtime 0, owner 0.
#[test]
fn the_file_is_reproducible() {
    let fx = fixture();
    let (a, b) = (fx.tmp.path().join("a.tar"), fx.tmp.path().join("b.tar"));
    bundle_of(&fx, &a);
    bundle_of(&fx, &b);
    assert_eq!(std::fs::read(a).unwrap(), std::fs::read(b).unwrap());
}

/// The rule of #16: a bundle reaches `pinned` when this build has the digests compiled in, and is
/// refused otherwise, and a mirror directory of the same files gives the same answer each time.
#[test]
fn a_bundle_is_pinned_or_refused_like_a_mirror() {
    let fx = fixture();
    let tar = fx.tmp.path().join("b.tar");
    bundle_of(&fx, &tar);
    let (_keep, bundle) = unpacked(&fx, &tar);
    let mirror = MirrorSource::parse(&fx.mirror_file_url()).unwrap();
    let m = manifest_of(&fx, "stable");
    let get = |src: &dyn Source, t: &DigestTrust| {
        get_artifact(src, t, &m, "cig", &v("1.0.0"), &fx.home, &mut NoProgress)
    };

    // no pins, the default single-source rule: both refused with the same code
    let strict = strict_trust();
    for src in [&bundle as &dyn Source, &mirror] {
        let e = get(src, &strict).unwrap_err();
        assert_eq!(e.code(), "E1822", "{}: {e}", src.name());
        assert!(downloads(&fx).is_empty());
    }

    // the opt-in accepts both at checksummed, and says it was not independent
    for src in [&bundle as &dyn Source, &mirror] {
        let s = get(src, &trust()).unwrap();
        assert_eq!(s.verified.level, TrustLevel::Checksummed);
        assert!(!s.verified.independent);
    }

    // with the digest compiled in, both are pinned and no opt-in is needed
    let pinned = DigestTrust::new(TrustLevel::Checksummed)
        .with_pins(pin_table(&fx))
        .with_attest(Arc::new(cigi_source::NoAttest));
    for src in [&bundle as &dyn Source, &mirror] {
        let s = get(src, &pinned).unwrap();
        assert_eq!(s.verified.level, TrustLevel::Pinned, "{}", src.name());
        assert!(
            !s.verified.independent,
            "pinned does not make it independent"
        );
        assert_eq!(s.origin, src.name());
    }
    // a pin the bundle's build disagrees with is an attack, not a downgrade
    let wrong: &'static [(&'static str, &'static str)] = Box::leak(
        vec![(
            Box::leak(format!("cig-1.0.0-{}", Target::host()).into_boxed_str()) as &'static str,
            "0000000000000000000000000000000000000000000000000000000000000000",
        )]
        .into_boxed_slice(),
    );
    let t = DigestTrust::new(TrustLevel::Checksummed)
        .with_pins(wrong)
        .with_attest(Arc::new(cigi_source::NoAttest));
    assert_eq!(get(&bundle, &t).unwrap_err().code(), "E1821");

    // the manifest comes out of the bundle admitted, like any other
    let a = get_manifest(&bundle, &strict, &fx.home, "stable", false).unwrap();
    assert_eq!(a.sequence, 1);
}

fn tar_with(build: impl FnOnce(&mut tar::Builder<Vec<u8>>)) -> Vec<u8> {
    let mut b = tar::Builder::new(Vec::new());
    build(&mut b);
    b.into_inner().unwrap()
}

fn add(b: &mut tar::Builder<Vec<u8>>, name: &str, body: &[u8], kind: tar::EntryType) {
    let mut h = tar::Header::new_gnu();
    h.set_size(body.len() as u64);
    h.set_mode(0o644);
    h.set_entry_type(kind);
    if kind == tar::EntryType::Symlink {
        h.set_link_name("/etc/passwd").unwrap();
    }
    b.append_data(&mut h, name, body).unwrap();
}

fn index_for(files: &[(&str, &[u8])]) -> Vec<u8> {
    let rows: Vec<serde_json::Value> = files
        .iter()
        .map(|(n, b)| {
            serde_json::json!({"name": n, "size": b.len(), "sha256": cigi_source::digest::sha256_hex(b)})
        })
        .collect();
    serde_json::to_vec(&serde_json::json!({"format": 1, "files": rows})).unwrap()
}

fn try_unpack(bytes: &[u8]) -> (Result<cigi_source::Unpacked, BundleError>, Vec<String>) {
    let tmp = tempfile::tempdir().unwrap();
    let tar = tmp.path().join("x.tar");
    std::fs::write(&tar, bytes).unwrap();
    let dest = tmp.path().join("dest");
    std::fs::create_dir(&dest).unwrap();
    let r = unpack_bundle(&tar, &dest);
    // nothing outside dest, whatever the entry said
    let outside: Vec<String> = std::fs::read_dir(tmp.path())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != "x.tar" && n != "dest")
        .collect();
    (r, outside)
}

#[test]
fn unpacking_refuses_everything_but_plain_files() {
    let good = b"hello".as_slice();
    let idx = index_for(&[("a.txt", good)]);
    let ok_tar = tar_with(|b| {
        add(b, "bundle.json", &idx, tar::EntryType::Regular);
        add(b, "a.txt", good, tar::EntryType::Regular);
    });
    assert!(try_unpack(&ok_tar).0.is_ok());

    let cases: Vec<(&str, Vec<u8>)> = vec![
        (
            "a symlink",
            tar_with(|b| {
                add(b, "bundle.json", &idx, tar::EntryType::Regular);
                add(b, "a.txt", b"", tar::EntryType::Symlink);
            }),
        ),
        (
            "a directory",
            tar_with(|b| {
                add(b, "bundle.json", &idx, tar::EntryType::Regular);
                add(b, "sub", b"", tar::EntryType::Directory);
            }),
        ),
        (
            "a path with a slash",
            tar_with(|b| {
                add(b, "bundle.json", &idx, tar::EntryType::Regular);
                add(b, "sub/a.txt", good, tar::EntryType::Regular);
            }),
        ),
        (
            "a hidden name",
            tar_with(|b| {
                add(b, "bundle.json", &idx, tar::EntryType::Regular);
                add(b, ".a.txt", good, tar::EntryType::Regular);
            }),
        ),
        (
            "the same name twice",
            tar_with(|b| {
                add(b, "bundle.json", &idx, tar::EntryType::Regular);
                add(b, "a.txt", good, tar::EntryType::Regular);
                add(b, "a.txt", good, tar::EntryType::Regular);
            }),
        ),
        (
            "a pax header",
            tar_with(|b| {
                b.append_pax_extensions([("comment", b"x".as_slice())])
                    .unwrap();
                add(b, "bundle.json", &idx, tar::EntryType::Regular);
                add(b, "a.txt", good, tar::EntryType::Regular);
            }),
        ),
    ];
    for (what, bytes) in cases {
        let (r, outside) = try_unpack(&bytes);
        let e = r.expect_err(what);
        assert_eq!(e.code(), "E1837", "{what}: {e}");
        assert!(outside.is_empty(), "{what}: {outside:?}");
    }
}

#[test]
fn the_index_must_agree_with_the_files() {
    let good = b"hello".as_slice();
    let cases: Vec<(&str, Vec<u8>)> = vec![
        (
            "no index",
            tar_with(|b| add(b, "a.txt", good, tar::EntryType::Regular)),
        ),
        (
            "an index that is not json",
            tar_with(|b| {
                add(b, "bundle.json", b"{", tar::EntryType::Regular);
            }),
        ),
        (
            "a file the index does not list",
            tar_with(|b| {
                add(b, "bundle.json", &index_for(&[]), tar::EntryType::Regular);
                add(b, "a.txt", good, tar::EntryType::Regular);
            }),
        ),
        (
            "a listed file that is not there",
            tar_with(|b| {
                add(
                    b,
                    "bundle.json",
                    &index_for(&[("a.txt", good)]),
                    tar::EntryType::Regular,
                );
            }),
        ),
        (
            "a file whose bytes differ from the index",
            tar_with(|b| {
                add(
                    b,
                    "bundle.json",
                    &index_for(&[("a.txt", good)]),
                    tar::EntryType::Regular,
                );
                add(b, "a.txt", b"HELLO", tar::EntryType::Regular);
            }),
        ),
    ];
    for (what, bytes) in cases {
        let (r, _) = try_unpack(&bytes);
        assert_eq!(r.expect_err(what).code(), "E1837", "{what}");
    }
    // not a tar at all
    let (r, _) = try_unpack(&[9u8; 1500]);
    assert_eq!(r.unwrap_err().code(), "E1837");
}

#[test]
fn the_writer_checks_what_it_writes() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("payload");
    std::fs::write(&src, b"abc").unwrap();
    let out = tmp.path().join("o.tar");
    // declared digest is not the file's: refused, nothing left
    let bad = BundleFile {
        name: "p.bin".into(),
        data: cigi_source::BundleData::File(src.clone()),
        sha256: "0".repeat(64),
        size: 3,
    };
    let e = write_bundle(&out, &[], vec![bad]).unwrap_err();
    assert_eq!(e.code(), "E1839", "{e}");
    assert!(!out.exists());
    assert_eq!(
        std::fs::read_dir(tmp.path()).unwrap().count(),
        1,
        "partial left"
    );
    // a name the layout cannot hold
    let e = write_bundle(&out, &[], vec![BundleFile::bytes("../x", vec![1])]).unwrap_err();
    assert_eq!(e.code(), "E1839");
    let e = write_bundle(&out, &[], vec![BundleFile::bytes("bundle.json", vec![1])]).unwrap_err();
    assert_eq!(e.code(), "E1839");
}

/// Measured, not assumed: `cargo test -p cigi-source --test bundle -- --ignored --nocapture`.
#[test]
#[ignore]
fn throughput() {
    let tmp = tempfile::tempdir().unwrap();
    let big = tmp.path().join("big");
    let body: Vec<u8> = (0..64 * 1024 * 1024u64)
        .map(|i| ((i * 2654435761) >> 7) as u8)
        .collect();
    std::fs::write(&big, &body).unwrap();
    let f = BundleFile {
        name: "big.bin".into(),
        sha256: cigi_source::digest::sha256_hex(&body),
        size: body.len() as u64,
        data: cigi_source::BundleData::File(big),
    };
    let out = tmp.path().join("big.tar");
    let t = std::time::Instant::now();
    write_bundle(&out, &[], vec![f]).unwrap();
    let w = t.elapsed();
    let dest = tmp.path().join("d");
    std::fs::create_dir(&dest).unwrap();
    let t = std::time::Instant::now();
    unpack_bundle(&out, &dest).unwrap();
    let r = t.elapsed();
    let mb = body.len() as f64 / 1_048_576.0;
    println!(
        "write {:.0} MB/s ({:?}), unpack {:.0} MB/s ({:?}) for {mb:.0} MB",
        mb / w.as_secs_f64(),
        w,
        mb / r.as_secs_f64(),
        r
    );
}
