// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! The release prune's dry run (#26).

use std::collections::BTreeMap;

use cig_compat::Version;
use xtask::assemble::{AssetInfo, ReleaseInfo, Releases, TagInfo};
use xtask::prune::{self, Options};

struct Fake(BTreeMap<String, Vec<String>>);

impl Releases for Fake {
    fn release(&self, repo: &str, tag: &str) -> Result<ReleaseInfo, String> {
        Err(format!("the prune plan never reads {repo} {tag}"))
    }
    fn small_asset(&self, _: &str, _: &AssetInfo, _: u64) -> Result<Vec<u8>, String> {
        Err("the prune plan never downloads".into())
    }
    fn tags(&self, repo: &str) -> Result<Vec<TagInfo>, String> {
        Ok(self.0[repo]
            .iter()
            .map(|t| TagInfo {
                tag: t.clone(),
                prerelease: false,
            })
            .collect())
    }
}

const TOOLS: &str = r#"
[tool.cig]
repo = "otmof-ops/cig-script"
tag = "v{version}"
layout = "binary"
asset = "cig-v{version}-{os}-{arch}{exe}"
shims = [ { name = "cig", path = "bin/cig" } ]
targets = [ "linux-x86_64" ]

[tool.cigo]
repo = "otmof-ops/cig-orchestrator"
tag = "v{version}"
layout = "archive"
asset = "cig-orchestrator-v{version}.tar.gz"
strip = 1
shims = [ { name = "cigo", path = "bin/cigo" } ]
targets = [ "any" ]
"#;

fn releases() -> Fake {
    let tags = |n: u64| (0..=n).map(|m| format!("v1.{m}.0")).collect::<Vec<_>>();
    Fake(BTreeMap::from([
        ("otmof-ops/cig-script".to_string(), tags(20)),
        ("otmof-ops/cig-orchestrator".to_string(), tags(3)),
    ]))
}

fn opts() -> Options {
    Options {
        keep: 3,
        product: None,
        pins: BTreeMap::new(),
    }
}

#[test]
fn prune_dry_run_lists_every_version_with_a_reason() {
    let tools = prune::tools_of(TOOLS).unwrap();
    let mut o = opts();
    o.pins.insert(
        ("cig".into(), Version::parse("1.15.0").unwrap()),
        vec!["cig-compile 0.2.0".into()],
    );
    let plans = prune::plan(&releases(), &tools, &o).unwrap();
    let cig = plans.iter().find(|p| p.product == "cig").unwrap();
    assert_eq!(cig.rows.len(), 21, "every version has a row");
    assert!(cig.rows.iter().all(|r| !r.reason.is_empty()));
    let keep: Vec<String> = cig
        .rows
        .iter()
        .filter(|r| r.keep)
        .map(|r| r.version.to_string())
        .collect();
    assert_eq!(keep, ["1.20.0", "1.19.0", "1.18.0", "1.15.0"]);
    // a product with fewer than three versions keeps them all
    let cigo = plans.iter().find(|p| p.product == "cigo").unwrap();
    assert_eq!(cigo.rows.len(), 4);
    assert_eq!(cigo.rows.iter().filter(|r| !r.keep).count(), 1);

    let text = prune::render(&plans, 3);
    assert_eq!(
        text.lines().filter(|l| l.contains("archive  ")).count(),
        17 + 1
    );
    assert!(text.contains("keep     1.15.0"), "{text}");
    assert!(text.contains("pinned by cig-compile 0.2.0"));
    assert!(text.contains("older than the 3 latest and not pinned"));
    assert!(text.ends_with("nothing was archived, deleted or uploaded\n"));
}

#[test]
fn one_product_can_be_asked_for_and_a_wrong_one_is_refused() {
    let tools = prune::tools_of(TOOLS).unwrap();
    let mut o = opts();
    o.product = Some("cigo".into());
    let plans = prune::plan(&releases(), &tools, &o).unwrap();
    assert_eq!(plans.len(), 1);
    o.product = Some("nope".into());
    assert!(prune::plan(&releases(), &tools, &o).is_err());
}

#[test]
fn pins_come_from_cartons_and_from_the_command_line() {
    let ((tool, v), who) = prune::parse_pin("cig=1.15.0:cig-compile 0.2.0 Cargo tag").unwrap();
    assert_eq!((tool.as_str(), v.to_string().as_str()), ("cig", "1.15.0"));
    assert_eq!(who, "cig-compile 0.2.0 Cargo tag");
    assert!(prune::parse_pin("cig=1.15.0").is_err());
    assert!(prune::parse_pin("cig=x:why").is_err());
    assert!(prune::YES_BLOCKED.contains("write and sign the index"));
    assert!(prune::HELP.contains("write and sign the index"));
}
