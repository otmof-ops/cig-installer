// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0

mod common;
use cig_compat::{ChangeKind, Manifest, Pin, Target, Yankable};
use common::*;

#[test]
fn the_spec_example_parses_and_round_trips() {
    let m = spec_manifest();
    assert_eq!((m.format, m.channel.as_str(), m.sequence), (1, "stable", 7));
    assert_eq!(m.generated, "2026-11-12T10:00:00Z");
    assert_eq!(m.tools.len(), 4);
    assert_eq!(m.cartons.len(), 1);
    assert!(m.warnings().is_empty());
    let again = Manifest::from_str(&m.render()).expect("rendered text parses");
    assert_eq!(again, m);
}

#[test]
fn which_cigo_is_in_carton_0_1_0() {
    let m = spec_manifest();
    assert_eq!(m.build_in(&v("0.1.0"), "cigo"), Some(&v("1.17.0")));
    assert_eq!(m.build_in(&v("0.1.0"), "cig"), Some(&v("1.20.0")));
    assert_eq!(m.build_in(&v("0.1.0"), "nope"), None);
    assert_eq!(m.build_in(&v("0.2.0"), "cigo"), None);
}

#[test]
fn latest_and_newest_matching() {
    let m = fleet();
    assert_eq!(m.latest(false).map(|c| c.version.clone()), Some(v("1.1.2")));
    let one = Pin::parse("1").unwrap();
    assert_eq!(m.newest_matching(&one, None).unwrap().version, v("1.1.2"));
    let installed = [v("1.0.0")];
    assert_eq!(
        m.newest_matching(&one, Some(&installed)).unwrap().version,
        v("1.0.0")
    );
    // a non-yanked carton beats a yanked one; only an exact pin still finds a yanked one
    let zero = Pin::parse("0.9").unwrap();
    assert!(m.newest_matching(&zero, None).is_none());
    assert!(m.newest_matching(&Pin::parse("0").unwrap(), None).is_none());
    let exact = Pin::parse("0.9.0").unwrap();
    assert_eq!(m.newest_matching(&exact, None).unwrap().version, v("0.9.0"));
    assert!(m.newest_matching(&Pin::parse("7").unwrap(), None).is_none());
    assert!(m.newest_matching(&one, Some(&[])).is_none());
    let m = spec_manifest();
    assert_eq!(
        m.newest_matching(&Pin::parse("0.1").unwrap(), None)
            .unwrap()
            .version,
        v("0.1.0")
    );
    assert_eq!(m.latest(false).unwrap().version, v("0.1.0"));
}

#[test]
fn latest_skips_yanked_and_prereleases() {
    let text = fleet_text()
        .replace(
            "[carton.\"1.1.2\"]",
            "[carton.\"1.2.0-rc.1\"]\ncig = \"1.8.0\"\ncigo = \"1.4.0\"\n[carton.\"1.1.2\"]",
        )
        .replace("[yanked]", "[yanked]\n\"carton 1.1.2\" = \"bad\"");
    let m = Manifest::from_str(&text).unwrap();
    assert_eq!(m.latest(false).unwrap().version, v("1.0.0"));
    assert_eq!(m.latest(true).unwrap().version, v("1.2.0-rc.1"));
}

#[test]
fn artifacts_and_names() {
    let m = spec_manifest();
    let a = m
        .artifact(
            "cig",
            &v("1.20.0"),
            &Target {
                os: "linux".into(),
                arch: "x86_64".into(),
            },
        )
        .unwrap();
    assert_eq!(a.name(), "cig-v1.20.0-linux-x86_64");
    assert_eq!(a.size, 13_010_720);
    assert_eq!(
        a.download_url,
        "https://github.com/otmof-ops/cig-script/releases/download/v1.20.0/cig-v1.20.0-linux-x86_64"
    );
    assert_eq!(a.sidecar, "cig-v1.20.0-linux-x86_64.sha256");
    assert_eq!(a.sidecar_url, format!("{}.sha256", a.download_url));
    assert!(m
        .artifact(
            "cig",
            &v("1.20.0"),
            &Target {
                os: "macos".into(),
                arch: "aarch64".into()
            }
        )
        .is_none());
    let any = Target {
        os: "freebsd".into(),
        arch: "arm".into(),
    };
    let c = m.artifact("cigo", &v("1.17.0"), &any).unwrap();
    assert_eq!(c.target, "any");
    assert_eq!(c.name(), "cig-orchestrator-v1.17.0.tar.gz");
    assert_eq!(c.strip, 1);
    let i = m
        .artifact(
            "ciginstaller",
            &v("0.1.0"),
            &Target {
                os: "linux".into(),
                arch: "x86_64".into(),
            },
        )
        .unwrap();
    assert_eq!(
        i.download_url,
        "https://example.invalid/ciginstaller-v0.1.0-linux-x86_64.tar.gz"
    );
    assert_eq!(
        Target {
            os: "linux".into(),
            arch: "x86_64".into()
        }
        .to_string(),
        "linux-x86_64"
    );
    assert!(!Target::host().os.is_empty());
}

#[test]
fn windows_assets_get_an_exe() {
    let text = SPEC_EXAMPLE.replace(
        "[build.cigo.\"1.17.0\"]",
        "[build.cig.\"1.20.0\".windows-x86_64]\nsha256 = \"3333333333333333333333333333333333333333333333333333333333333333\"\nsize = 5\n[build.cigo.\"1.17.0\"]",
    );
    let m = Manifest::from_str(&text).unwrap();
    let a = m
        .artifact(
            "cig",
            &v("1.20.0"),
            &Target {
                os: "windows".into(),
                arch: "x86_64".into(),
            },
        )
        .unwrap();
    assert_eq!(a.name(), "cig-v1.20.0-windows-x86_64.exe");
}

#[test]
fn packs() {
    let m = spec_manifest();
    assert_eq!(m.pack(&v("0.1.0"), "dev").unwrap(), ["cig", "cigo"]);
    assert_eq!(m.pack(&v("0.1.0"), "builder").unwrap().len(), 3);
    assert!(m.pack(&v("0.1.0"), "security").is_none());
}

#[test]
fn diff_lists_only_differences() {
    let m = fleet();
    let d = m.diff(&v("1.0.0"), &v("1.1.2"));
    assert_eq!(d.len(), 2);
    assert_eq!(
        (d[0].tool.as_str(), d[0].kind()),
        ("cig", ChangeKind::Upgraded)
    );
    assert_eq!(d[1].from, Some(v("1.3.0")));
    assert_eq!(d[1].to, Some(v("1.4.0")));
    let back = m.diff(&v("1.1.2"), &v("1.0.0"));
    assert_eq!(back[0].kind(), ChangeKind::Downgraded);
    assert!(m.diff(&v("1.0.0"), &v("1.0.0")).is_empty());
    assert!(m.diff(&v("1.0.0"), &v("9.9.9")).is_empty());
    let both = m.diff(&v("0.9.0"), &v("1.0.0"));
    assert_eq!(both.len(), 1);
}

#[test]
fn yanks() {
    let m = spec_manifest();
    let y = m.yank_of(Yankable::Carton(v("0.0.9"))).unwrap();
    assert!(y.reason.starts_with("cigo could skip"));
    let y = m
        .yank_of(Yankable::Product("cigo".into(), v("1.4.1")))
        .unwrap();
    assert_eq!(
        (y.reason.as_str(), y.fixed_in.clone()),
        ("example", Some(v("1.4.2")))
    );
    assert!(m.yank_of(Yankable::Carton(v("0.1.0"))).is_none());
}

#[test]
fn product_requirements() {
    let m = spec_manifest();
    let p = m.product("cigcompile", &v("0.2.0")).unwrap();
    assert_eq!(p.journal_format(), Some(3));
    assert_eq!(p.cig_runtime(), Some(v("1.18.0")));
    assert_eq!(p.os_min["linux"], "kernel 3.10");
    let r = &m.product("cigo", &v("1.17.0")).unwrap().requires["cig"];
    assert!(
        r.matches(&v("1.16.1"))
            && r.matches(&v("1.20.0"))
            && !r.matches(&v("2.0.0"))
            && !r.matches(&v("1.16.0"))
    );
}

#[test]
fn expiry() {
    let m = spec_manifest();
    let at = cig_compat::rfc3339_to_epoch("2026-12-12T10:00:00Z").unwrap();
    assert!(!m.is_expired(at - 1));
    assert!(m.is_expired(at));
}

#[test]
fn unknown_keys_are_kept_and_warned_about() {
    let text = format!("{SPEC_EXAMPLE}\n[future]\nthing = 1\n")
        .replace("attests = false", "attests = false\nmystery = \"x\"");
    let m = Manifest::from_str(&text).unwrap();
    assert!(m.extra.contains_key("future"));
    assert_eq!(m.warnings().len(), 2);
    assert_eq!(Manifest::from_str(&m.render()).unwrap(), m);
}

// ---- section 6.2: one test per rule, all E1825

fn refused(text: &str) -> String {
    let e = Manifest::from_str(text).expect_err("should be refused");
    assert_eq!(e.code, "E1825", "{e}");
    e.message
}

fn edit(from: &str, to: &str) -> String {
    assert!(SPEC_EXAMPLE.contains(from), "fixture lacks {from:?}");
    SPEC_EXAMPLE.replacen(from, to, 1)
}

#[test]
fn rule_format_is_one() {
    assert!(refused(&edit("format    = 1", "format = 2")).contains("newer"));
    assert!(refused(&edit("format    = 1\n", "")).contains("format"));
    assert!(refused(&edit("format    = 1", "format = \"1\"")).contains("format"));
}

#[test]
fn rule_sequence_present() {
    assert!(refused(&edit("sequence  = 7\n", "")).contains("sequence"));
    assert!(refused(&edit("sequence  = 7", "sequence = -1")).contains("sequence"));
}

#[test]
fn rule_expires_after_generated() {
    assert!(refused(&edit(
        "expires   = 2026-12-12T10:00:00Z",
        "expires = 2026-11-12T10:00:00Z"
    ))
    .contains("after"));
    assert!(refused(&edit(
        "expires   = 2026-12-12T10:00:00Z",
        "expires = 2026-11-01T10:00:00Z"
    ))
    .contains("after"));
    assert!(refused(&edit(
        "expires   = 2026-12-12T10:00:00Z",
        "expires = 2026-12-12"
    ))
    .contains("date-times"));
}

#[test]
fn rule_carton_versions_parse() {
    refused(&edit("[carton.\"0.1.0\"]", "[carton.\"0.1\"]"));
    refused(&edit(
        "[carton.\"0.1.0\".packs]",
        "[carton.\"0.1.0\".packs]\nx = 1",
    ));
}

#[test]
fn rule_product_pin_needs_a_build() {
    assert!(refused(&edit("cigo         = \"1.17.0\"", "cigo = \"1.16.0\"")).contains("build"));
}

#[test]
fn rule_reserved_keys_only() {
    assert!(
        refused(&edit("tested       = []", "tested = []\nbogus = \"1.0.0\"")).contains("bogus")
    );
    refused(&edit("tested       = []", "tested = \"no\""));
}

#[test]
fn rule_sha256_is_64_lowercase_hex() {
    let good = "4649fda09236ada8a2ade2460c33db9cfa4b2f4dba40186099118a6a3b2dc8e3";
    refused(&edit(good, &good.to_uppercase()));
    refused(&edit(good, &good[..63]));
    refused(&edit(good, &format!("{good}0")));
    refused(&edit(good, &format!("g{}", &good[1..])));
}

#[test]
fn rule_no_duplicate_cartons() {
    let text = edit(
        "[product.cigo.\"1.17.0\"]",
        "[carton.\"v0.1.0\"]\ncig = \"1.20.0\"\n[product.cigo.\"1.17.0\"]",
    );
    assert!(refused(&text).contains("duplicate"));
}

#[test]
fn rule_requires_ranges_parse() {
    assert!(refused(&edit(">=1.16.1, <2", "not a range")).contains("range"));
}

#[test]
fn rule_sizes_below_1_gb() {
    refused(&edit("size   = 13010720", "size = 1000000000"));
    refused(&edit("size   = 13010720", "size = -1"));
    assert!(Manifest::from_str(&edit("size   = 13010720", "size = 999999999")).is_ok());
}

#[test]
fn rule_packs() {
    refused(&edit("dev     =", "Dev =")); // bad name
    refused(&edit("[ \"cig\", \"cigo\" ]", "[ \"cig\", \"nothere\" ]"));
    refused(&edit("[ \"cig\", \"cigo\" ]", "\"cig\""));
}

#[test]
fn rule_builds_match_declared_tools_and_targets() {
    refused(&edit(
        "[build.cigo.\"1.17.0\"]",
        "[build.ghost.\"1.0.0\"]\n[build.cigo.\"1.17.0\"]",
    ));
    refused(&edit(
        "[build.cig.\"1.20.0\".linux-x86_64]",
        "[build.cig.\"1.20.0\".plan9-mips]",
    ));
    // a missing target is allowed (that platform cannot install this carton)
    assert!(Manifest::from_str(SPEC_EXAMPLE).is_ok());
}

#[test]
fn rule_templates_must_fill() {
    refused(&edit(
        "cig-orchestrator-v{version}.tar.gz",
        "cigo-{os}.tar.gz",
    ));
    refused(&edit(
        "asset   = \"cig-orchestrator-v{version}.tar.gz\"",
        "asset = \"x-{nope}\"",
    ));
}

#[test]
fn rule_tools_are_well_formed() {
    refused(&edit(
        "layout  = \"binary\"\nasset   = \"cig-v",
        "layout = \"zip\"\nasset = \"cig-v",
    ));
    refused(&edit(
        "shims   = [ { name = \"cig\", path = \"bin/cig\", windows_path = \"bin/cig.exe\" } ]",
        "shims = []",
    ));
    refused(&edit("targets = [ \"any\" ]", "targets = []"));
    refused(&edit("targets = [ \"any\" ]", "targets = [ \"nodash\" ]"));
}

#[test]
fn rule_yanks_and_advisories_are_well_formed() {
    refused(&edit("\"carton 0.0.9\"", "\"carton 0.0\""));
    refused(&edit("\"cigo 1.4.1\"", "\"ghost 1.4.1\""));
    refused(&edit("severity = \"security\"", "severity = \"scary\""));
    refused(&edit("[ \"carton <0.1.0\" ]", "[ \"carton nonsense\" ]"));
    refused(&edit("fixed_in = \"1.4.2\"", "fixed_in = \"1.4\""));
}

#[test]
fn rule_toml_must_be_toml() {
    refused("this is = not [ toml");
    refused("");
    refused(&edit("channel   = \"stable\"", "channel = \"\""));
}

#[test]
fn oversize_text_is_refused() {
    let big = format!("{SPEC_EXAMPLE}\n# {}", "x".repeat(17 * 1024 * 1024));
    assert!(refused(&big).contains("16 MiB"));
}

#[test]
fn rule_yank_needs_a_reason() {
    refused(&edit("\"carton 0.0.9\" = \"cigo could skip a file on rollback after a box apply; use 0.1.0 or later\"", "\"carton 0.0.9\" = \"\""));
}

#[test]
fn rule_shim_paths_are_plain_relative_paths() {
    let good =
        "shims   = [ { name = \"cig\", path = \"bin/cig\", windows_path = \"bin/cig.exe\" } ]";
    for bad in [
        "../../../../bin/sh",
        "/bin/sh",
        "bin/../../x",
        "bin//cig",
        "./cig",
        "bin\\\\cig",
        "C:cig",
        "",
    ] {
        let shim = format!("shims   = [ {{ name = \"cig\", path = \"{bad}\" }} ]");
        let e = refused(&edit(good, &shim));
        assert!(e.contains("shims[0].path"), "{bad:?}: {e}");
        let shim = format!(
            "shims   = [ {{ name = \"cig\", path = \"bin/cig\", windows_path = \"{bad}\" }} ]"
        );
        let e = refused(&edit(good, &shim));
        assert!(e.contains("windows_path"), "{bad:?}: {e}");
    }
    assert!(Manifest::from_str(SPEC_EXAMPLE).is_ok());
}

#[test]
fn rule_shim_names_are_plain_command_names() {
    // a shim's name becomes a file in bin/, first on PATH (SEC-004, PR #37)
    for bad in [
        "../../victim",
        "a/b",
        "..",
        "Cig",
        "",
        "-x",
        "ciginstaller",
        "cigi",
    ] {
        let text = edit(
            r#"{ name = "cig", path = "bin/cig","#,
            &format!(r#"{{ name = {bad:?}, path = "bin/cig","#),
        );
        let e = Manifest::from_str(&text).expect_err(bad);
        assert_eq!(e.code, "E1825", "{bad:?}: {e}");
    }
    let ok = edit(
        r#"{ name = "cig", path = "bin/cig","#,
        r#"{ name = "cig-2", path = "bin/cig","#,
    );
    assert!(Manifest::from_str(&ok).is_ok());
}

#[test]
fn rule_two_tools_never_share_a_shim() {
    let text = edit(
        r#"{ name = "cigo", path = "bin/cigo","#,
        r#"{ name = "cig", path = "bin/cigo","#,
    );
    assert!(refused(&text).contains("already"), "{text}");
}
