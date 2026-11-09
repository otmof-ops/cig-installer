// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0

use super::*;
use std::cell::RefCell;

const TOOLS: &str = r#"
[tool.cig]
repo = "otmof-ops/cig-script"
tag = "v{version}"
layout = "binary"
asset = "cig-v{version}-{os}-{arch}{exe}"
sidecar = "{asset}.sha256"
shims = [ { name = "cig", path = "bin/cig" } ]
targets = [ "linux-x86_64", "macos-aarch64" ]

[tool.cigo]
repo = "otmof-ops/cig-orchestrator"
tag = "v{version}"
layout = "archive"
asset = "cig-orchestrator-v{version}.tar.gz"
sidecar = "{asset}.sha256"
strip = 1
shims = [ { name = "cigo", path = "bin/cigo" } ]
targets = [ "any" ]
"#;

const PRODUCTS: &str = r#"
[product.cig."1.20.0"]
requires = {}
[product.cig."1.5.0"]
requires = {}
[product.cig."1.21.0"]
requires = {}
[product.cigo."1.17.0"]
requires = { cig = ">=1.16.1, <2" }
"#;

/// Releases in memory; counts the release lookups so carried-forward builds can be proved unfetched.
#[derive(Default)]
struct Fake {
    releases: BTreeMap<(String, String), ReleaseInfo>,
    blobs: BTreeMap<u64, Vec<u8>>,
    lookups: RefCell<Vec<String>>,
}

impl Fake {
    fn publish(&mut self, repo: &str, tag: &str, prerelease: bool) -> &mut Self {
        self.releases.insert(
            (repo.into(), tag.into()),
            ReleaseInfo {
                published: "2026-11-09".into(),
                prerelease,
                assets: BTreeMap::new(),
            },
        );
        self
    }

    fn file(&mut self, repo: &str, tag: &str, name: &str, bytes: &[u8], digest: Option<String>) {
        let id = self.blobs.len() as u64 + 1;
        self.blobs.insert(id, bytes.to_vec());
        let r = self.releases.get_mut(&(repo.into(), tag.into())).unwrap();
        r.assets.insert(
            name.into(),
            AssetInfo {
                id,
                size: bytes.len() as u64,
                digest,
            },
        );
    }

    /// An asset, its sidecar, and GitHub's digest of it.
    fn asset(&mut self, repo: &str, tag: &str, name: &str, bytes: &[u8]) -> String {
        let d = hex::encode(Sha256::digest(bytes));
        self.file(repo, tag, name, bytes, Some(d.clone()));
        let side = format!("{d}  {name}\n");
        let sd = hex::encode(Sha256::digest(side.as_bytes()));
        self.file(
            repo,
            tag,
            &format!("{name}.sha256"),
            side.as_bytes(),
            Some(sd),
        );
        d
    }
}

impl Releases for Fake {
    fn release(&self, repo: &str, tag: &str) -> Result<ReleaseInfo, String> {
        self.lookups.borrow_mut().push(format!("{repo} {tag}"));
        self.releases
            .get(&(repo.to_string(), tag.to_string()))
            .cloned()
            .ok_or_else(|| format!("{repo} has no published release {tag}"))
    }
    fn small_asset(&self, _repo: &str, asset: &AssetInfo, max: u64) -> Result<Vec<u8>, String> {
        let b = self.blobs.get(&asset.id).cloned().ok_or("no such asset")?;
        if b.len() as u64 > max {
            return Err("too big".into());
        }
        Ok(b)
    }
}

/// cig 1.20.0 (linux only), cig 1.21.0, cig 1.5.0 and cigo 1.17.0, published.
fn fake() -> (Fake, String) {
    let mut f = Fake::default();
    f.publish("otmof-ops/cig-script", "v1.20.0", false);
    let cig = f.asset(
        "otmof-ops/cig-script",
        "v1.20.0",
        "cig-v1.20.0-linux-x86_64",
        b"cig 1.20.0 linux",
    );
    for v in ["1.21.0", "1.5.0"] {
        let tag = format!("v{v}");
        f.publish("otmof-ops/cig-script", &tag, false);
        f.asset(
            "otmof-ops/cig-script",
            &tag,
            &format!("cig-v{v}-linux-x86_64"),
            v.as_bytes(),
        );
    }
    f.publish("otmof-ops/cig-orchestrator", "v1.17.0", false);
    f.asset(
        "otmof-ops/cig-orchestrator",
        "v1.17.0",
        "cig-orchestrator-v1.17.0.tar.gz",
        b"cigo tarball",
    );
    (f, cig)
}

fn v(s: &str) -> Version {
    Version::parse(s).unwrap()
}

fn req(carton: &str, members: &[(&str, &str)]) -> Request {
    Request {
        channel: "stable".into(),
        carton: v(carton),
        members: members
            .iter()
            .map(|(t, ver)| (t.to_string(), v(ver)))
            .collect(),
        packs: BTreeMap::new(),
        notes: String::new(),
        expires_days: 30,
        previous: None,
        tools_toml: TOOLS.into(),
        products_toml: PRODUCTS.into(),
        now: OffsetDateTime::from_unix_timestamp(1_794_000_000).unwrap(),
    }
}

#[test]
fn the_first_manifest_of_a_channel() {
    let (f, cig) = fake();
    let mut r = req("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.17.0")]);
    r.packs
        .insert("dev".into(), vec!["cig".into(), "cigo".into()]);
    r.notes = "first carton".into();
    let a = assemble(&r, &f).unwrap();
    let m = &a.manifest;
    assert_eq!((m.sequence, m.channel.as_str()), (1, "stable"));
    let c = m.carton(&v("0.1.0")).unwrap();
    assert_eq!(c.products.get("cigo"), Some(&v("1.17.0")));
    assert_eq!(c.packs.get("dev").map(Vec::len), Some(2));
    assert!(c.tested.is_empty());
    let b = &m.builds[&("cig".to_string(), v("1.20.0"))];
    assert_eq!(b.artifacts["linux-x86_64"].sha256, cig);
    assert_eq!(b.artifacts["linux-x86_64"].size, 16);
    assert!(!b.artifacts.contains_key("macos-aarch64"));
    assert!(a
        .report
        .iter()
        .any(|l| l.contains("no build for macos-aarch64")));
    assert!(a.report.iter().any(|l| l.contains("tested = []")));
    // what was written is what a client reads, and the sidecar is of that text
    assert_eq!(&Manifest::from_str(&a.text).unwrap(), m);
    assert_eq!(a.sha256, hex::encode(Sha256::digest(a.text.as_bytes())));
    assert_eq!(
        sidecar_line(&a.sha256, "manifest-stable.toml"),
        format!("{}  manifest-stable.toml\n", a.sha256)
    );
}

#[test]
fn the_next_manifest_carries_the_last_one_forward() {
    let (f, _) = fake();
    let first = assemble(&req("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.17.0")]), &f).unwrap();
    f.lookups.borrow_mut().clear();
    let mut r = req("0.2.0", &[("cig", "1.21.0"), ("cigo", "1.17.0")]);
    r.previous = Some(first.text.clone());
    let next = assemble(&r, &f).unwrap();
    assert_eq!(next.manifest.sequence, 2);
    // the old carton is untouched, the published cigo build is not fetched again
    assert_eq!(
        next.manifest.carton(&v("0.1.0")),
        first.manifest.carton(&v("0.1.0"))
    );
    assert_eq!(
        *f.lookups.borrow(),
        vec!["otmof-ops/cig-script v1.21.0".to_string()]
    );
    assert!(next
        .report
        .iter()
        .any(|l| l.contains("cigo 1.17.0: published before")));

    // a carton number is never reused
    let mut again = req("0.2.0", &[("cig", "1.21.0")]);
    again.previous = Some(next.text.clone());
    assert!(assemble(&again, &f)
        .unwrap_err()
        .contains("already published"));
    // nor a channel mixed up
    let mut other = req("0.3.0", &[("cig", "1.21.0")]);
    other.previous = Some(next.text.clone());
    other.channel = "beta".into();
    assert!(assemble(&other, &f).unwrap_err().contains("channel"));
}

#[test]
fn requirements_must_hold_inside_the_carton() {
    let (f, _) = fake();
    let e = assemble(&req("0.1.0", &[("cig", "1.5.0"), ("cigo", "1.17.0")]), &f).unwrap_err();
    assert!(e.contains("cigo 1.17.0 needs cig >=1.16.1, <2"), "{e}");
    assert!(e.contains("has cig 1.5.0"), "{e}");
    let e = assemble(&req("0.1.0", &[("cigo", "1.17.0")]), &f).unwrap_err();
    assert!(e.contains("has no cig"), "{e}");
}

#[test]
fn every_product_needs_its_requirements_written() {
    let (mut f, _) = fake();
    f.publish("otmof-ops/cig-script", "v1.22.0", false);
    f.asset(
        "otmof-ops/cig-script",
        "v1.22.0",
        "cig-v1.22.0-linux-x86_64",
        b"x",
    );
    let e = assemble(&req("0.1.0", &[("cig", "1.22.0")]), &f).unwrap_err();
    assert!(e.contains("[product.cig.\"1.22.0\"]"), "{e}");
}

#[test]
fn a_sidecar_must_exist_and_agree() {
    // no sidecar
    let (mut f, _) = fake();
    let tag = ("otmof-ops/cig-script".to_string(), "v1.20.0".to_string());
    f.releases
        .get_mut(&tag)
        .unwrap()
        .assets
        .remove("cig-v1.20.0-linux-x86_64.sha256");
    let e = assemble(&req("0.1.0", &[("cig", "1.20.0")]), &f).unwrap_err();
    assert!(e.contains("has no sidecar"), "{e}");

    // a sidecar GitHub's digest disagrees with
    let (mut f, _) = fake();
    f.releases
        .get_mut(&tag)
        .unwrap()
        .assets
        .get_mut("cig-v1.20.0-linux-x86_64")
        .unwrap()
        .digest = Some("0".repeat(64));
    let e = assemble(&req("0.1.0", &[("cig", "1.20.0")]), &f).unwrap_err();
    assert!(e.contains("GitHub's own digest"), "{e}");

    // a sidecar that names another file
    let (mut f, _) = fake();
    let id = f.releases[&tag].assets["cig-v1.20.0-linux-x86_64.sha256"].id;
    let d = hex::encode(Sha256::digest(b"cig 1.20.0 linux"));
    f.blobs
        .insert(id, format!("{d}  something-else\n").into_bytes());
    let e = assemble(&req("0.1.0", &[("cig", "1.20.0")]), &f).unwrap_err();
    assert!(e.contains("names something-else"), "{e}");

    // a sidecar that is not a digest (an HTML error page)
    let (mut f, _) = fake();
    f.blobs.insert(id, b"<html>not found</html>".to_vec());
    assert!(assemble(&req("0.1.0", &[("cig", "1.20.0")]), &f).is_err());

    // no digest from GitHub: the sidecar alone, and the report says so
    let (mut f, _) = fake();
    f.releases
        .get_mut(&tag)
        .unwrap()
        .assets
        .get_mut("cig-v1.20.0-linux-x86_64")
        .unwrap()
        .digest = None;
    let a = assemble(&req("0.1.0", &[("cig", "1.20.0")]), &f).unwrap();
    assert!(a.report.iter().any(|l| l.contains("sidecar alone")));
}

#[test]
fn yanked_versions_and_changed_definitions_are_refused() {
    let (f, _) = fake();
    let first = assemble(&req("0.1.0", &[("cig", "1.20.0")]), &f).unwrap();
    let yanked = format!("{}\n[yanked]\n\"cig 1.20.0\" = \"bad build\"\n", first.text);
    let mut r = req("0.2.0", &[("cig", "1.20.0")]);
    r.previous = Some(yanked);
    assert!(assemble(&r, &f).unwrap_err().contains("yanked"));

    // a published build's names never change
    let mut r = req("0.2.0", &[("cig", "1.21.0")]);
    r.previous = Some(first.text.clone());
    r.tools_toml = TOOLS.replace("cig-v{version}-{os}", "cig-{version}-{os}");
    assert!(assemble(&r, &f).unwrap_err().contains("never change"));
    // a new target is fine
    let mut r = req("0.2.0", &[("cig", "1.21.0")]);
    r.previous = Some(first.text);
    r.tools_toml = TOOLS.replace(
        r#"targets = [ "linux-x86_64", "macos-aarch64" ]"#,
        r#"targets = [ "linux-x86_64", "linux-aarch64", "macos-aarch64" ]"#,
    );
    assert!(assemble(&r, &f).is_ok());
}

#[test]
fn the_stable_channel_takes_no_prerelease() {
    let (mut f, _) = fake();
    f.publish("otmof-ops/cig-script", "v1.21.0", true);
    f.asset(
        "otmof-ops/cig-script",
        "v1.21.0",
        "cig-v1.21.0-linux-x86_64",
        b"rc",
    );
    let e = assemble(&req("0.1.0", &[("cig", "1.21.0")]), &f).unwrap_err();
    assert!(e.contains("prerelease"), "{e}");
    let mut beta = req("0.1.0", &[("cig", "1.21.0")]);
    beta.channel = "beta".into();
    assert!(assemble(&beta, &f).is_ok());
}

#[test]
fn expiry_is_one_to_ninety_days() {
    let (f, _) = fake();
    for days in [0, 91, -1] {
        let mut r = req("0.1.0", &[("cig", "1.20.0")]);
        r.expires_days = days;
        assert!(assemble(&r, &f).unwrap_err().contains("expires"), "{days}");
    }
    let mut r = req("0.1.0", &[("cig", "1.20.0")]);
    r.expires_days = 90;
    let a = assemble(&r, &f).unwrap();
    let span = a.manifest.expires_epoch().unwrap()
        - cig_compat::rfc3339_to_epoch(&a.manifest.generated).unwrap();
    assert_eq!(span, 90 * 86_400);
}

#[test]
fn the_tracked_inputs_parse_with_the_clients_parser() {
    let tools = include_str!("../../../manifest/tools.toml");
    let products = include_str!("../../../manifest/products.toml");
    let mut r = req("0.1.0", &[("cig", "1.20.0")]);
    r.tools_toml = tools.into();
    r.products_toml = products.into();
    let m = definitions(&r, "2026-11-10T00:00:00Z", "2026-12-10T00:00:00Z").unwrap();
    for t in ["cig", "cigo", "cigcompile", "ciginstaller"] {
        assert!(m.tools.contains_key(t), "{t}");
    }
    // the real asset names, as the releases have them
    let cig = &m.tools["cig"];
    assert_eq!(
        cig.names(&v("1.20.0"), "windows-x86_64").unwrap().asset,
        "cig-v1.20.0-windows-x86_64.exe"
    );
    assert_eq!(
        m.tools["cigo"].names(&v("1.17.0"), "any").unwrap().sidecar,
        "cig-orchestrator-v1.17.0.tar.gz.sha256"
    );
    assert!(m.product("cigo", &v("1.17.0")).is_some());
}

#[test]
fn pins_come_from_every_build() {
    let (f, cig) = fake();
    let a = assemble(&req("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.17.0")]), &f).unwrap();
    let pins = crate::pins::pins_for(&a.text).unwrap();
    assert!(pins.contains(&format!("(\"cig-1.20.0-linux-x86_64\", \"{cig}\")")));
    assert!(pins.contains("\"cigo-1.17.0-any\""));
    assert!(pins.starts_with("// SPDX-FileCopyrightText"));
}
