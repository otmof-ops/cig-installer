// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! Phase 5 acceptance (#16): `bundle`, `install --from <file.tar>`, `mirror`.
//!
//! The fixture's releases folder is one source, so every install here is single-source: the
//! tests that need it to succeed say `CIGI_ALLOW_SINGLE_SOURCE` the way `common::cigi` does, and
//! the ones about the default do not. Bundles and mirrors are admitted by the same rules.

mod common;

use std::path::{Path, PathBuf};
use std::process::Output;

use cigi_source::{unpack_bundle, write_bundle, BundleFile};
use cigi_testkit::{Fixture, Snapshot};
use common::{cigi, fixture, ok, text};

fn host() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

fn two_cartons(fx: &mut Fixture) {
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.16.0")]);
    fx.carton("0.2.0", &[("cig", "1.21.0"), ("cigo", "1.17.0")]);
    fx.publish("stable");
}

/// A second machine: its own home, and a mirror that is nowhere (`install --from` replaces the
/// source, so nothing may be asked of it).
fn offline(fx2: &Fixture, allow: bool, args: &[&str]) -> Output {
    let mut c = fx2.cigi_with_mirror("file:///nowhere-at-all");
    if allow {
        c.env("CIGI_ALLOW_SINGLE_SOURCE", "true");
    }
    c.arg("--yes")
        .args(args)
        .output()
        .expect("run ciginstaller")
}

fn json_of(o: &Output) -> serde_json::Value {
    ok(o);
    serde_json::from_slice(&o.stdout).expect("json")
}

fn bundle(fx: &Fixture, carton: &str, out: &Path) -> Output {
    cigi(
        fx,
        &[
            "bundle",
            "--carton",
            carton,
            "--out",
            &out.to_string_lossy(),
        ],
    )
}

/// The files of a toolchain, receipts left out (they name when and from where).
fn toolchain_bytes(fx: &Fixture) -> Snapshot {
    let s = Snapshot::take(&fx.cigscript().join("toolchains"));
    Snapshot {
        entries: s
            .entries
            .into_iter()
            .filter(|(k, _)| !k.ends_with(".cigi-receipt.json") && !k.starts_with("ciginstaller"))
            .collect(),
    }
}

fn tool_dirs(fx: &Fixture) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(fx.cigscript().join("toolchains"))
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    v.retain(|n| n != "ciginstaller");
    v.sort();
    v
}

fn assert_untouched(fx: &Fixture, why: &str) {
    let root = fx.cigscript();
    for p in ["toolchains", "bin", "humidor/store/objects", "humidor"] {
        let s = Snapshot::take(&root.join(p)).without(&["store"]);
        assert!(
            s.entries.is_empty(),
            "{why}: {p} holds {:?}",
            s.entries.keys().collect::<Vec<_>>()
        );
    }
    assert!(!root.join("installer/state.json").exists(), "{why}");
    assert!(!root.join("installer/shim-index").exists(), "{why}");
    let leftovers: Vec<String> = std::fs::read_dir(root.join("installer/cache"))
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.starts_with("bundle-"))
                .collect()
        })
        .unwrap_or_default();
    assert!(leftovers.is_empty(), "{why}: left {leftovers:?}");
}

/// Take a bundle apart and put it back together with `f` applied to each file, the index
/// recomputed: what someone who can edit the file and knows the format would hand over.
fn rebuilt(from: &Path, to: &Path, f: impl Fn(&str, Vec<u8>) -> Vec<u8>) {
    let scratch = tempfile::tempdir().unwrap();
    let un = unpack_bundle(from, scratch.path()).unwrap();
    let files: Vec<BundleFile> = un
        .files
        .iter()
        .map(|(n, _)| {
            let bytes = std::fs::read(scratch.path().join(n)).unwrap();
            BundleFile::bytes(n.clone(), f(n, bytes))
        })
        .collect();
    let meta: Vec<(&str, serde_json::Value)> = ["channel", "sequence", "carton", "targets"]
        .iter()
        .map(|k| (*k, un.index[*k].clone()))
        .collect();
    write_bundle(to, &meta, files).unwrap();
}

/// #16's done-when: a bundle made in one home installs in another with no network, and the
/// toolchain bytes are the ones a mirror install makes.
#[test]
fn a5_bundle_round_trip() {
    let mut fx = fixture();
    two_cartons(&mut fx);
    // the reference: this home installs from the fixture's mirror
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));

    let tar = fx.tmp.path().join("carton-0.1.0.tar");
    let o = cigi(
        &fx,
        &[
            "--json",
            "bundle",
            "--carton",
            "0.1.0",
            "--targets",
            &host(),
            "--out",
            &tar.to_string_lossy(),
        ],
    );
    let v = json_of(&o);
    assert_eq!(v["data"]["carton"], "0.1.0");
    assert_eq!(v["data"]["signed"], false);
    assert!(tar.is_file());
    let names: Vec<String> = v["data"]["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["name"].as_str().unwrap().to_string())
        .collect();
    assert!(
        names.contains(&"manifest-stable.toml".to_string()),
        "{names:?}"
    );
    assert!(
        names.contains(&"manifest-stable.toml.sha256".to_string()),
        "{names:?}"
    );
    assert!(
        names.iter().filter(|n| n.ends_with(".sha256")).count() >= 3,
        "an asset sidecar for each build: {names:?}"
    );
    // the human output says what it is
    let o = cigi(
        &fx,
        &[
            "bundle",
            "--carton",
            "0.1.0",
            "--out",
            &tar.to_string_lossy(),
        ],
    );
    ok(&o);
    assert!(text(&o).contains("unsigned"), "{}", text(&o));

    // a second machine, no network, nothing but the file
    let fx2 = fixture();
    let o = offline(
        &fx2,
        true,
        &["install", "carton", "--from", &tar.to_string_lossy()],
    );
    ok(&o);
    let t = text(&o);
    assert!(t.contains("carton 0.1.0 installed"), "{t}");
    assert!(t.contains("a bundle"), "{t}");
    assert!(t.contains("unsigned"), "{t}");
    assert_eq!(tool_dirs(&fx2), vec!["cig", "cigo"]);
    assert_eq!(
        toolchain_bytes(&fx).entries.len(),
        toolchain_bytes(&fx2).entries.len()
    );
    let (a, b) = (toolchain_bytes(&fx), toolchain_bytes(&fx2));
    assert!(
        a.diff(&b).is_empty(),
        "toolchain bytes differ: {:?}",
        a.diff(&b)
    );
    // it runs
    let run = fx2.run_shim("cigo", &fx2.project, &[]);
    ok(&run);
    // the unpacked copy is gone, and the receipt names the bundle and one source
    let leftovers = std::fs::read_dir(fx2.cigscript().join("installer/cache"))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("bundle-"))
        .count();
    assert_eq!(leftovers, 0);
    let rc = std::fs::read_to_string(
        fx2.cigscript()
            .join("toolchains/cigo/1.16.0/.cigi-receipt.json"),
    )
    .unwrap();
    assert!(rc.contains("bundle:carton-0.1.0.tar"), "{rc}");
    assert!(rc.contains("\"independent\": false"), "{rc}");

    // again: nothing to do, and still no network
    let o = offline(
        &fx2,
        true,
        &["install", "carton", "--from", &tar.to_string_lossy()],
    );
    ok(&o);
    assert!(text(&o).contains("already installed"), "{}", text(&o));

    // the doctor says it was one source, and that a bundle is unsigned
    let o = fx2.cigi().args(["doctor"]).output().unwrap();
    let t = text(&o);
    assert!(t.contains("one source"), "{t}");
    assert!(t.contains("bundle carton-0.1.0.tar"), "{t}");
    assert!(t.contains("unsigned until #9"), "{t}");
}

/// The same inputs give the same bytes, so a second run can be compared with the first.
#[test]
fn a5_bundle_is_reproducible() {
    let mut fx = fixture();
    two_cartons(&mut fx);
    let (a, b) = (fx.tmp.path().join("a.tar"), fx.tmp.path().join("b.tar"));
    ok(&bundle(&fx, "0.2.0", &a));
    ok(&bundle(&fx, "0.2.0", &b));
    assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
    // and no partial file is left beside it
    let stray: Vec<_> = std::fs::read_dir(fx.tmp.path())
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().contains("partial"))
        .collect();
    assert!(stray.is_empty(), "{stray:?}");
}

/// A bundle is one source, like a mirror: without `allow_single_source` it is refused, exactly
/// as the mirror directory it was made from is, and nothing is installed.
#[test]
fn a5_bundle_is_one_source_and_refused_like_a_mirror() {
    let mut fx = fixture();
    two_cartons(&mut fx);
    let tar = fx.tmp.path().join("c.tar");
    ok(&bundle(&fx, "0.1.0", &tar));

    let fx_bundle = fixture();
    let o = offline(
        &fx_bundle,
        false,
        &["install", "carton", "--from", &tar.to_string_lossy()],
    );
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(text(&o).contains("E1822"), "{}", text(&o));
    assert_untouched(&fx_bundle, "bundle");

    let fx_dir = fixture();
    let o = offline(
        &fx_dir,
        false,
        &[
            "install",
            "carton",
            "--from",
            &fx.releases.to_string_lossy(),
        ],
    );
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(text(&o).contains("E1822"), "{}", text(&o));
    assert_untouched(&fx_dir, "mirror directory");

    // no flag raises or lowers anything: --require pinned on an unpinned build is refused, and
    // refused the way the mirror directory is
    let fx_req = fixture();
    let o = offline(
        &fx_req,
        true,
        &[
            "install",
            "carton",
            "--require",
            "pinned",
            "--from",
            &tar.to_string_lossy(),
        ],
    );
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert_untouched(&fx_req, "require pinned");
    let fx_req_dir = fixture();
    let o2 = offline(
        &fx_req_dir,
        true,
        &[
            "install",
            "carton",
            "--require",
            "pinned",
            "--from",
            &fx.releases.to_string_lossy(),
        ],
    );
    assert_eq!(o2.status.code(), Some(2), "{}", text(&o2));
    let code = |o: &Output| {
        text(o)
            .split_whitespace()
            .find(|w| w.contains("error[E"))
            .map(String::from)
    };
    assert_eq!(code(&o), code(&o2), "{}\n{}", text(&o), text(&o2));
}

/// Tampering inside the bundle, with the index left alone and with the index made to agree:
/// the first is caught at unpacking, the second by the manifest's digests and the sidecars.
#[test]
fn a5_bundle_tampering_is_refused() {
    let mut fx = fixture();
    two_cartons(&mut fx);
    let tar = fx.tmp.path().join("good.tar");
    ok(&bundle(&fx, "0.1.0", &tar));
    let asset = std::fs::read(
        fx.releases.join(
            // the cig build is a single file named by the fixture
            std::fs::read_dir(&fx.releases)
                .unwrap()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .find(|n| n.starts_with("cig-v1.20.0") && !n.ends_with(".sha256"))
                .expect("the cig 1.20.0 asset"),
        ),
    )
    .unwrap();

    // 1. a byte flipped in the artifact, the index untouched
    let raw = std::fs::read(&tar).unwrap();
    let at = raw
        .windows(asset.len().min(64))
        .position(|w| w == &asset[..asset.len().min(64)])
        .expect("the asset is in the tar");
    let mut bad = raw.clone();
    bad[at + 10] ^= 0xff;
    let flipped = fx.tmp.path().join("flipped.tar");
    std::fs::write(&flipped, bad).unwrap();
    let fx2 = fixture();
    let o = offline(
        &fx2,
        true,
        &["install", "carton", "--from", &flipped.to_string_lossy()],
    );
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(text(&o).contains("E1837"), "{}", text(&o));
    assert_untouched(&fx2, "flipped byte");

    // 2. the artifact replaced and the index made to agree: the manifest's digest says no
    let swapped = fx.tmp.path().join("swapped.tar");
    rebuilt(&tar, &swapped, |n, mut b| {
        if n.starts_with("cig-v1.20.0") && !n.ends_with(".sha256") {
            b.push(b'!');
        }
        b
    });
    let fx3 = fixture();
    let o = offline(
        &fx3,
        true,
        &["install", "carton", "--from", &swapped.to_string_lossy()],
    );
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    let t = text(&o);
    assert!(t.contains("E1821") || t.contains("E1827"), "{t}");
    assert_untouched(&fx3, "swapped artifact");

    // 3. the manifest edited and the index made to agree: its sidecar says no
    let edited = fx.tmp.path().join("edited.tar");
    rebuilt(&tar, &edited, |n, b| {
        if n == "manifest-stable.toml" {
            let mut t = String::from_utf8(b).unwrap();
            t.push_str("\n# edited\n");
            t.into_bytes()
        } else {
            b
        }
    });
    let fx4 = fixture();
    let o = offline(
        &fx4,
        true,
        &["install", "carton", "--from", &edited.to_string_lossy()],
    );
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(text(&o).contains("E1827"), "{}", text(&o));
    assert_untouched(&fx4, "edited manifest");

    // 4. the manifest and its sidecar both replaced, naming a different digest for the cig build:
    //    the build in the bundle no longer matches what the manifest says
    let real = cigi_testkit::sha256_hex(&asset);
    let forged = std::fs::read_to_string(fx.releases.join("manifest-stable.toml"))
        .unwrap()
        .replace(&real, &"0".repeat(64));
    assert_ne!(forged.len(), 0);
    let both = fx.tmp.path().join("both.tar");
    rebuilt(&tar, &both, |n, b| match n {
        "manifest-stable.toml" => forged.clone().into_bytes(),
        "manifest-stable.toml.sha256" => format!(
            "{}  manifest-stable.toml\n",
            cigi_testkit::sha256_hex(forged.as_bytes())
        )
        .into_bytes(),
        _ => b,
    });
    let fx5 = fixture();
    let o = offline(
        &fx5,
        true,
        &["install", "carton", "--from", &both.to_string_lossy()],
    );
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    let t = text(&o);
    assert!(t.contains("E1821") || t.contains("E1827"), "{t}");
    assert_untouched(&fx5, "forged manifest");

    // 5. not a bundle at all
    let junk = fx.tmp.path().join("junk.tar");
    std::fs::write(&junk, vec![7u8; 2048]).unwrap();
    let fx6 = fixture();
    let o = offline(
        &fx6,
        true,
        &["install", "carton", "--from", &junk.to_string_lossy()],
    );
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(text(&o).contains("E1837"), "{}", text(&o));
    assert_untouched(&fx6, "junk");
}

/// A target the carton has no build for is refused, and no file is written.
#[test]
fn a5_bundle_needs_a_build_for_every_target() {
    let mut fx = fixture();
    two_cartons(&mut fx);
    let tar = fx.tmp.path().join("t.tar");
    let o = cigi(
        &fx,
        &[
            "bundle",
            "--carton",
            "0.1.0",
            "--targets",
            "plan9-mips",
            "--out",
            &tar.to_string_lossy(),
        ],
    );
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(text(&o).contains("E1815"), "{}", text(&o));
    assert!(!tar.exists());
    // an output folder that is not there is E1839, before any download
    let o = cigi(
        &fx,
        &[
            "bundle",
            "--out",
            &fx.tmp.path().join("no/such/dir/x.tar").to_string_lossy(),
        ],
    );
    assert!(text(&o).contains("E1839"), "{}", text(&o));
    // and a directory cannot be the output
    let o = cigi(&fx, &["bundle", "--out", &fx.tmp.path().to_string_lossy()]);
    assert!(text(&o).contains("E1839"), "{}", text(&o));
}

fn dir_names(d: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(d)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

/// `mirror <dir>` fills a directory a team can serve; an install reads it; running it again only
/// adds, and removes nothing it did not put there.
#[test]
fn a5_mirror_fills_and_only_adds() {
    let mut fx = fixture();
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.16.0")]);
    fx.publish("stable");
    let mirror: PathBuf = fx.tmp.path().join("team-mirror");

    let o = cigi(&fx, &["--json", "mirror", &mirror.to_string_lossy()]);
    let v = json_of(&o);
    let first = dir_names(&mirror);
    assert!(
        first.contains(&"manifest-stable.toml".to_string()),
        "{first:?}"
    );
    assert!(
        first.contains(&"manifest-stable.toml.sha256".to_string()),
        "{first:?}"
    );
    assert!(
        v["data"]["added"].as_array().unwrap().len() >= 2,
        "{}",
        v["data"]
    );
    // what it wrote is what the source held, byte for byte
    for n in &first {
        assert_eq!(
            std::fs::read(mirror.join(n)).unwrap(),
            std::fs::read(fx.releases.join(n)).unwrap(),
            "{n}"
        );
    }

    // a second machine installs from the directory, by --from and by CIGI_MIRROR
    let fx2 = fixture();
    let o = offline(
        &fx2,
        true,
        &["install", "carton", "--from", &mirror.to_string_lossy()],
    );
    ok(&o);
    assert_eq!(tool_dirs(&fx2), vec!["cig", "cigo"]);
    let fx3 = fixture();
    let mut c = fx3.cigi_with_mirror(&format!("file://{}", mirror.display()));
    c.env("CIGI_ALLOW_SINGLE_SOURCE", "true");
    ok(&c.args(["--yes", "install", "carton"]).output().unwrap());
    assert!(toolchain_bytes(&fx2)
        .diff(&toolchain_bytes(&fx3))
        .is_empty());

    // a stray file of someone else's, then a re-run: nothing added, nothing touched
    std::fs::write(mirror.join("notes.txt"), "mine\n").unwrap();
    let before = Snapshot::take(&mirror);
    let o = cigi(&fx, &["--json", "mirror", &mirror.to_string_lossy()]);
    let v = json_of(&o);
    assert_eq!(
        v["data"]["added"].as_array().unwrap().len(),
        0,
        "{}",
        v["data"]
    );
    assert_eq!(v["data"]["manifest_written"], false);
    let after = Snapshot::take(&mirror);
    assert!(before.diff(&after).is_empty(), "{:?}", before.diff(&after));

    // a newer carton is published: the run adds it and keeps every old file and the stray one
    fx.carton("0.2.0", &[("cig", "1.21.0"), ("cigo", "1.17.0")]);
    fx.publish("stable");
    let o = cigi(&fx, &["--json", "mirror", &mirror.to_string_lossy()]);
    let v = json_of(&o);
    assert!(
        v["data"]["added"].as_array().unwrap().len() >= 2,
        "{}",
        v["data"]
    );
    assert_eq!(v["data"]["manifest_written"], true);
    let now = Snapshot::take(&mirror);
    for (p, e) in &before.entries {
        if p == "manifest-stable.toml" || p == "manifest-stable.toml.sha256" {
            continue;
        }
        assert_eq!(now.entries.get(p), Some(e), "{p} changed or went");
    }
    assert!(mirror.join("notes.txt").is_file());
    // and the new carton installs from it
    let fx4 = fixture();
    ok(&offline(
        &fx4,
        true,
        &[
            "install",
            "carton",
            "--carton",
            "0.2.0",
            "--from",
            &mirror.to_string_lossy(),
        ],
    ));
}

/// A damaged file in a mirror is put right on the next run, and nothing else changes.
#[test]
fn a5_mirror_rerun_repairs_a_damaged_file() {
    let mut fx = fixture();
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.16.0")]);
    fx.publish("stable");
    let mirror = fx.tmp.path().join("m");
    ok(&cigi(&fx, &["mirror", &mirror.to_string_lossy()]));
    let victim = dir_names(&mirror)
        .into_iter()
        .find(|n| n.starts_with("cig-v1.20.0") && !n.ends_with(".sha256"))
        .unwrap();
    std::fs::write(mirror.join(&victim), b"damaged").unwrap();
    let o = cigi(&fx, &["--json", "mirror", &mirror.to_string_lossy()]);
    let v = json_of(&o);
    assert_eq!(
        v["data"]["added"].as_array().unwrap().len(),
        1,
        "{}",
        v["data"]
    );
    assert_eq!(
        std::fs::read(mirror.join(&victim)).unwrap(),
        std::fs::read(fx.releases.join(&victim)).unwrap()
    );
}

/// A mirror never goes backwards, never fills itself from itself, and never lives in the home.
#[test]
fn a5_mirror_refusals() {
    let mut fx_new = fixture();
    fx_new.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.16.0")]);
    fx_new.publish("stable");
    fx_new.publish("stable"); // sequence 2
    let mirror = fx_new.tmp.path().join("m");
    ok(&cigi(&fx_new, &["mirror", &mirror.to_string_lossy()]));

    // another machine whose source is behind (sequence 1)
    let mut fx_old = fixture();
    fx_old.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.16.0")]);
    fx_old.publish("stable");
    let before = Snapshot::take(&mirror);
    let o = cigi(&fx_old, &["mirror", &mirror.to_string_lossy()]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(text(&o).contains("E1838"), "{}", text(&o));
    assert!(before.diff(&Snapshot::take(&mirror)).is_empty());

    // the directory this run reads from
    let o = cigi(&fx_new, &["mirror", &fx_new.releases.to_string_lossy()]);
    assert!(text(&o).contains("E1838"), "{}", text(&o));
    // inside the home
    let inside = fx_new.cigscript().join("mirror");
    let o = cigi(&fx_new, &["mirror", &inside.to_string_lossy()]);
    assert!(text(&o).contains("E1838"), "{}", text(&o));
    // a file where the directory should be
    let file = fx_new.tmp.path().join("afile");
    std::fs::write(&file, "x").unwrap();
    let o = cigi(&fx_new, &["mirror", &file.to_string_lossy()]);
    assert!(text(&o).contains("E1838"), "{}", text(&o));
}

/// Only the asked-for carton's builds go in, and a bundle made from a mirror made by `mirror`
/// installs too: the three forms are one layout.
#[test]
fn a5_mirror_one_carton_then_bundle_from_it() {
    let mut fx = fixture();
    two_cartons(&mut fx);
    let mirror = fx.tmp.path().join("one");
    ok(&cigi(
        &fx,
        &[
            "mirror",
            &mirror.to_string_lossy(),
            "--carton",
            "0.1.0",
            "--channel",
            "stable",
        ],
    ));
    let names = dir_names(&mirror);
    assert!(
        names.iter().any(|n| n.starts_with("cig-v1.20.0")),
        "{names:?}"
    );
    assert!(
        !names.iter().any(|n| n.starts_with("cig-v1.21.0")),
        "{names:?}"
    );

    // a machine that reads that mirror makes a bundle, and a third installs it
    let fx_b = fixture();
    let mut c = fx_b.cigi_with_mirror(&format!("file://{}", mirror.display()));
    let tar = fx_b.tmp.path().join("from-mirror.tar");
    ok(&c
        .env("CIGI_ALLOW_SINGLE_SOURCE", "true")
        .args([
            "--yes",
            "bundle",
            "--carton",
            "0.1.0",
            "--out",
            &tar.to_string_lossy(),
        ])
        .output()
        .unwrap());
    let fx_c = fixture();
    ok(&offline(
        &fx_c,
        true,
        &["install", "carton", "--from", &tar.to_string_lossy()],
    ));
    assert_eq!(tool_dirs(&fx_c), vec!["cig", "cigo"]);
}
