// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

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
                notes: String::new(),
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
    fn tags(&self, repo: &str) -> Result<Vec<TagInfo>, String> {
        Ok(self
            .releases
            .iter()
            .filter(|((r, _), _)| r == repo)
            .map(|((_, tag), info)| TagInfo {
                tag: tag.clone(),
                prerelease: info.prerelease,
            })
            .collect())
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
    for t in ["cig", "cigo", "cigcompile", "cigar", "ciginstaller"] {
        assert!(m.tools.contains_key(t), "{t}");
    }
    // cigar's release workflow packs `cigar` and `cigar-lsp` as
    // `cigar-${GITHUB_REF_NAME}-<os>-<arch>.tar.gz`, and cigar 0.1.0 is its first release.
    let cigar = &m.tools["cigar"];
    let n = cigar.names(&v("0.1.0"), "windows-x86_64").unwrap();
    assert_eq!(n.asset, "cigar-v0.1.0-windows-x86_64.tar.gz");
    assert_eq!(n.sidecar, "cigar-v0.1.0-windows-x86_64.tar.gz.sha256");
    assert_eq!(
        cigar.names(&v("0.1.0"), "linux-x86_64").unwrap().asset,
        "cigar-v0.1.0-linux-x86_64.tar.gz"
    );
    assert_eq!(
        cigar.targets.len(),
        5,
        "the five targets its workflow builds"
    );
    assert!(!cigar
        .targets
        .iter()
        .any(|t| t == "linux-arm" || t == "freebsd-x86_64"));
    let shims: Vec<&str> = cigar.shims.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        shims,
        ["cigar", "cigar-lsp"],
        "a shim for each program in the archive"
    );
    assert!(cigar.attests);
    // cigar 0.1.0 is cigar's first release, standalone (cigscript at tag v1.20.0 as a library),
    // so a [product.cigar."0.1.0"] exists with no `requires` and FSL-1.1-ALv2 as its licence.
    let cigar_build = m
        .products
        .get(&("cigar".to_string(), v("0.1.0")))
        .expect("[product.cigar.\"0.1.0\"] is in manifest/products.toml");
    assert!(cigar_build.requires.is_empty(), "cigar never shells out to cig");
    assert_eq!(cigar_build.license.as_deref(), Some("FSL-1.1-ALv2"));
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
    // every tracked build states its license (an SPDX expression)
    for ((tool, ver), p) in &m.products {
        let l = p.license.as_deref().unwrap_or("");
        assert!(!l.is_empty(), "{tool} {ver} states no license");
    }
}

#[test]
fn a_products_license_reaches_the_manifest_and_a_missing_one_is_reported() {
    let (f, _) = fake();
    let mut r = req("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.17.0")]);
    r.products_toml = r#"
[product.cig."1.20.0"]
requires = {}
license = "Apache-2.0"
[product.cigo."1.17.0"]
requires = { cig = ">=1.16.1, <2" }
"#
    .into();
    let a = assemble(&r, &f).unwrap();
    assert_eq!(
        a.manifest
            .product("cig", &v("1.20.0"))
            .unwrap()
            .license
            .as_deref(),
        Some("Apache-2.0")
    );
    assert_eq!(
        a.manifest.product("cigo", &v("1.17.0")).unwrap().license,
        None
    );
    assert!(a
        .report
        .iter()
        .any(|l| l.contains("cigo 1.17.0") && l.contains("no `license`")));
    assert!(!a
        .report
        .iter()
        .any(|l| l.contains("cig 1.20.0") && l.contains("no `license`")));
    // it survives the render and the read
    let again = Manifest::from_str(&a.text).unwrap();
    assert_eq!(
        again
            .product("cig", &v("1.20.0"))
            .unwrap()
            .license
            .as_deref(),
        Some("Apache-2.0")
    );
    assert!(a.text.contains("license = \"Apache-2.0\""));
}

#[test]
fn a_license_that_is_not_an_spdx_expression_is_refused() {
    let (f, _) = fake();
    for bad in ["", "all rights reserved!", "Apache 2.0; see file"] {
        let mut r = req("0.1.0", &[("cig", "1.20.0")]);
        r.products_toml =
            format!("[product.cig.\"1.20.0\"]\nrequires = {{}}\nlicense = \"{bad}\"\n");
        assert!(assemble(&r, &f).is_err(), "`{bad}` was taken");
    }
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

/// A previous manifest of `channel` holding cartons with these numbers (no builds: only the
/// numbers matter to the numbering).
fn manifest_with_cartons(channel: &str, cartons: &[&str]) -> Manifest {
    let (f, _) = fake();
    let mut r = req(cartons[0], &[("cig", "1.20.0")]);
    r.channel = channel.into();
    let mut text = assemble(&r, &f).unwrap().text;
    for c in &cartons[1..] {
        let mut r = req(c, &[("cig", "1.20.0")]);
        r.channel = channel.into();
        r.previous = Some(text);
        text = assemble(&r, &f).unwrap().text;
    }
    Manifest::from_str(&text).unwrap()
}

#[test]
fn a_beta_is_numbered_one_past_the_last_beta_of_the_same_base() {
    let base = v("1.2.0");
    assert_eq!(
        next_candidate(&base, Kind::Beta, None).unwrap().to_string(),
        "1.2.0-beta.1"
    );
    // 10 is past 9 by value, not by text; other bases, nightlies and releases do not count
    let prev = manifest_with_cartons(
        "beta",
        &[
            "1.1.0-beta.40",
            "1.2.0-beta.1",
            "1.2.0-beta.9",
            "1.2.0-beta.10",
            "1.2.0-nightly.77",
            "1.2.0-rc.3",
            "1.2.0",
        ],
    );
    assert_eq!(
        next_candidate(&base, Kind::Beta, Some(&prev))
            .unwrap()
            .to_string(),
        "1.2.0-beta.11"
    );
    assert_eq!(
        next_candidate(&v("1.3.0"), Kind::Beta, Some(&prev))
            .unwrap()
            .to_string(),
        "1.3.0-beta.1"
    );
    assert_eq!(
        next_candidate(&v("1.1.0"), Kind::Beta, Some(&prev))
            .unwrap()
            .to_string(),
        "1.1.0-beta.41"
    );
}

#[test]
fn each_channel_keeps_its_own_floor() {
    let beta = manifest_with_cartons("beta", &["1.2.0-beta.5"]);
    let nightly = manifest_with_cartons("nightly", &["1.2.0-nightly.3", "1.2.0-beta.9"]);
    let base = v("1.2.0");
    assert_eq!(
        next_candidate(&base, Kind::Nightly, Some(&nightly))
            .unwrap()
            .to_string(),
        "1.2.0-nightly.4"
    );
    assert_eq!(
        next_candidate(&base, Kind::Beta, Some(&beta))
            .unwrap()
            .to_string(),
        "1.2.0-beta.6"
    );
    // a manifest of the other channel is refused, never counted
    let e = next_candidate(&base, Kind::Beta, Some(&nightly)).unwrap_err();
    assert!(e.contains("nightly channel's, not the beta"), "{e}");
    let e = next_candidate(&base, Kind::Nightly, Some(&beta)).unwrap_err();
    assert!(e.contains("beta channel's, not the nightly"), "{e}");
    // a base is a release number
    assert!(next_candidate(&v("1.2.0-beta.1"), Kind::Beta, None).is_err());
}

#[test]
fn a_tag_names_a_version_only_when_it_reads_back_exactly() {
    assert_eq!(version_from_tag("v{version}", "v1.20.0"), Some(v("1.20.0")));
    assert_eq!(
        version_from_tag("v{version}", "v1.22.0-beta.1"),
        Some(v("1.22.0-beta.1"))
    );
    assert_eq!(version_from_tag("{version}", "1.2.3"), Some(v("1.2.3")));
    assert_eq!(
        version_from_tag("cig-{version}-final", "cig-1.2.3-final"),
        Some(v("1.2.3"))
    );
    for (template, tag) in [
        ("v{version}", "1.20.0"),
        ("v{version}", "vv1.20.0"),
        ("v{version}", "v1.20"),
        ("v{version}", "nightly"),
        ("v{version}", "v1.20.0+build"),
        ("{version}", "v1.2.3"),
        ("v{version}{x}", "v1.2.3"),
        ("v", "v"),
    ] {
        assert_eq!(version_from_tag(template, tag), None, "{template} {tag}");
    }
}

fn tag(t: &str, prerelease: bool) -> TagInfo {
    TagInfo {
        tag: t.into(),
        prerelease,
    }
}

#[test]
fn the_newest_build_is_chosen_by_precedence_under_the_channels_policy() {
    let tags = [
        tag("v1.5.0", false),
        tag("v1.20.0", false),
        tag("v1.21.0", false),
        tag("v1.22.0-beta.2", true),
        tag("v1.22.0-beta.10", true),
        tag("v1.22.0-beta.9", true),
        tag("v2.0.0", true), // flagged on GitHub, so a prerelease whatever its number
        tag("nightly", true),
        tag("junk", false),
    ];
    let none = |_: &Version| false;
    assert_eq!(
        newest_build("v{version}", &tags, true, &none),
        Some(v("2.0.0"))
    );
    assert_eq!(
        newest_build("v{version}", &tags, false, &none),
        Some(v("1.21.0"))
    );
    // the yanked are never chosen, and neither is anything when nothing is eligible
    let yanked = |x: &Version| *x == v("2.0.0") || *x == v("1.22.0-beta.10");
    assert_eq!(
        newest_build("v{version}", &tags, true, &yanked),
        Some(v("1.22.0-beta.9"))
    );
    assert_eq!(newest_build("v{version}", &tags[..0], true, &none), None);
    assert_eq!(
        newest_build("v{version}", &[tag("v1.0.0-rc.1", false)], false, &none),
        None
    );
}

/// The products file with a prerelease of cig written in.
fn nightly_products() -> String {
    format!("{PRODUCTS}\n[product.cig.\"1.22.0-beta.1\"]\nrequires = {{}}\n")
}

#[test]
fn a_nightly_takes_every_tools_newest_build_and_gets_a_nightly_number() {
    let (mut f, _) = fake();
    f.publish("otmof-ops/cig-script", "v1.22.0-beta.1", true);
    f.asset(
        "otmof-ops/cig-script",
        "v1.22.0-beta.1",
        "cig-v1.22.0-beta.1-linux-x86_64",
        b"cig beta",
    );
    let mut r = req("1.2.0", &[]);
    r.channel = "nightly".into();
    r.products_toml = nightly_products();
    let (members, report) = latest_members(&r, &f, true).unwrap();
    assert_eq!(members["cig"], v("1.22.0-beta.1"));
    assert_eq!(members["cigo"], v("1.17.0"));
    assert!(report.iter().any(|l| l.contains("cig 1.22.0-beta.1")));
    // a tool the caller named is not replaced
    r.members.insert("cig".into(), v("1.20.0"));
    let (named, _) = latest_members(&r, &f, true).unwrap();
    assert_eq!(named["cig"], v("1.20.0"));
    // the stable policy takes no prerelease
    r.members.clear();
    let (stable, _) = latest_members(&r, &f, false).unwrap();
    assert_eq!(stable["cig"], v("1.21.0"));

    // semver ranges skip prereleases, so cigo's `cig >=1.16.1, <2` refuses a cig prerelease: a
    // nightly of prereleases fails loudly until products.toml writes a range that names them
    r.members = members;
    r.carton = next_candidate(&v("1.2.0"), Kind::Nightly, None).unwrap();
    let e = assemble(&r, &f).unwrap_err();
    assert!(e.contains("cigo 1.17.0 needs cig"), "{e}");

    // numbered, assembled, and read back by the client's parser (cig alone)
    f.releases
        .retain(|(repo, _), _| repo != "otmof-ops/cig-orchestrator");
    r.members.clear();
    let (members, _) = latest_members(&r, &f, true).unwrap();
    r.members = members;
    assert_eq!(r.members.len(), 1);
    r.carton = next_candidate(&v("1.2.0"), Kind::Nightly, None).unwrap();
    let first = assemble(&r, &f).unwrap();
    assert_eq!(r.carton.to_string(), "1.2.0-nightly.1");
    assert_eq!(first.manifest.channel, "nightly");
    assert_eq!(Manifest::from_str(&first.text).unwrap(), first.manifest);
    r.previous = Some(first.text.clone());
    r.members.clear();
    let prev = Manifest::from_str(&first.text).unwrap();
    r.carton = next_candidate(&v("1.2.0"), Kind::Nightly, Some(&prev)).unwrap();
    let (again, _) = latest_members(&r, &f, true).unwrap();
    r.members = again;
    let second = assemble(&r, &f).unwrap();
    assert_eq!(r.carton.to_string(), "1.2.0-nightly.2");
    // the channel's own sequence floor rises with every manifest
    assert_eq!((first.manifest.sequence, second.manifest.sequence), (1, 2));
}

#[test]
fn a_tool_with_nothing_eligible_is_left_out_and_said_so() {
    let (mut f, _) = fake();
    f.releases
        .retain(|(repo, _), _| repo != "otmof-ops/cig-orchestrator");
    let mut r = req("1.2.0", &[]);
    r.channel = "nightly".into();
    let (members, report) = latest_members(&r, &f, true).unwrap();
    assert!(!members.contains_key("cigo"));
    assert!(report
        .iter()
        .any(|l| l.contains("cigo: no eligible release")));
}

#[test]
fn the_stable_channel_takes_no_candidate_number() {
    let (f, _) = fake();
    let e = assemble(&req("1.2.0-beta.1", &[("cig", "1.20.0")]), &f).unwrap_err();
    assert!(e.contains("candidate number"), "{e}");
    let mut b = req("1.2.0-beta.1", &[("cig", "1.20.0")]);
    b.channel = "beta".into();
    assert!(assemble(&b, &f).is_ok());
}

#[test]
fn fixed_refs_reads_githubs_closing_keywords() {
    let notes =
        "## Fixed\n- Fixes #12: rollback skipped a file.\n- **Closes** otmof-ops/cig-script#7, \
                 and resolved: #3)\n- see #99 (not a fix) and fixes nothing#x\n- fixes #12 again";
    assert_eq!(
        fixed_refs(notes, "otmof-ops/cig-orchestrator"),
        vec![
            ("otmof-ops/cig-orchestrator".to_string(), 12),
            ("otmof-ops/cig-script".to_string(), 7),
            ("otmof-ops/cig-orchestrator".to_string(), 3),
        ]
    );
    assert!(fixed_refs("no references here", "o/r").is_empty());
}

#[test]
fn a_new_builds_release_notes_record_its_fixes_for_their_reporters() {
    let (mut f, _) = fake();
    f.releases
        .get_mut(&("otmof-ops/cig-orchestrator".into(), "v1.17.0".into()))
        .unwrap()
        .notes = "Fixes #12, and closes otmof-ops/cig-script#7.".into();
    let a = assemble(&req("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.17.0")]), &f).unwrap();
    let ids: Vec<(&str, String, usize)> = a
        .manifest
        .advisories
        .iter()
        .map(|x| {
            (
                x.id.as_str(),
                x.fixed_in
                    .as_ref()
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
                x.affects.len(),
            )
        })
        .collect();
    assert_eq!(
        ids,
        vec![
            ("otmof-ops/cig-orchestrator#12", "1.17.0".to_string(), 0),
            ("otmof-ops/cig-script#7", "1.17.0".to_string(), 0),
        ]
    );
    assert!(a
        .report
        .iter()
        .any(|l| l.contains("fixes otmof-ops/cig-orchestrator#12")));
    // the next carton carries them forward and adds none twice
    let mut next = req("0.2.0", &[("cig", "1.21.0"), ("cigo", "1.17.0")]);
    next.previous = Some(a.text.clone());
    let b = assemble(&next, &f).unwrap();
    assert_eq!(b.manifest.advisories.len(), 2);
}
