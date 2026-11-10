// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
#![allow(dead_code)]

use cig_compat::{Host, Manifest, Version};

pub const SPEC_EXAMPLE: &str = include_str!("../golden/spec-example.toml");

pub fn spec_manifest() -> Manifest {
    Manifest::from_str(SPEC_EXAMPLE).expect("the spec example parses")
}

pub fn v(s: &str) -> Version {
    Version::parse(s).expect("test version")
}

pub fn items(list: &[(&str, &str)]) -> Vec<(String, Version)> {
    list.iter().map(|(t, x)| (t.to_string(), v(x))).collect()
}

pub fn linux_host(kernel: &str) -> Host {
    Host {
        os: "linux".into(),
        arch: "x86_64".into(),
        os_version: None,
        kernel: Some(kernel.into()),
    }
}

const HEAD: &str = r#"
format = 1
channel = "stable"
sequence = 3
generated = 2026-11-01T00:00:00Z
expires = 2026-12-01T00:00:00Z

[tool.cig]
repo = "otmof-ops/cig-script"
tag = "v{version}"
layout = "binary"
asset = "cig-v{version}-{os}-{arch}{exe}"
shims = [ { name = "cig", path = "bin/cig" } ]
targets = [ "linux-x86_64", "macos-aarch64" ]

[tool.cigo]
repo = "otmof-ops/cig-orchestrator"
tag = "v{version}"
layout = "archive"
asset = "cig-orchestrator-v{version}.tar.gz"
strip = 1
shims = [ { name = "cigo", path = "bin/cigo" } ]
targets = [ "any" ]
"#;

fn build(tool: &str, version: &str, target: &str) -> String {
    format!(
        "[build.{tool}.\"{version}\"]\n[build.{tool}.\"{version}\".{target}]\nsha256 = \"{}\"\nsize = 1000\n",
        "a".repeat(64)
    )
}

/// A small channel: cartons 0.9.0 (yanked), 1.0.0 and 1.1.2, with the builds,
/// requirements, an advisory and an OS minimum that the `check` tests exercise.
pub fn fleet_text() -> String {
    let mut s = String::from(HEAD);
    for ver in ["1.6.0", "1.7.2", "1.8.0"] {
        s.push_str(&build("cig", ver, "linux-x86_64"));
    }
    for ver in ["1.3.0", "1.4.0"] {
        s.push_str(&build("cigo", ver, "any"));
    }
    s.push_str(
        r#"
[carton."0.9.0"]
cig = "1.6.0"
cigo = "1.3.0"
[carton."1.0.0"]
cig = "1.7.2"
cigo = "1.3.0"
packs = { dev = [ "cig", "cigo" ] }
[carton."1.1.2"]
cig = "1.8.0"
cigo = "1.4.0"

[product.cigo."1.4.0"]
requires = { cig = ">=1.8" }
[product.cigo."1.3.0"]
requires = { cig = ">=1.6, <2" }
recommends = { cig = ">=1.7" }
[product.cig."1.6.0"]
embeds = { journal_format = 2 }
[product.cig."1.7.2"]
embeds = { journal_format = 2 }
[product.cig."1.8.0"]
embeds = { journal_format = 3 }
os_min = { linux = "kernel 5.4", windows = "10" }

[yanked]
"carton 0.9.0" = { reason = "rollback skipped a file", fixed_in = "1.0.0" }
"cigo 1.4.1" = { reason = "bad launcher", fixed_in = "1.4.2" }

[[advisory]]
id = "CIGI-2026-0001"
affects = [ "carton <0.9.5" ]
fixed_in = "1.0.0"
severity = "security"
summary = "an example advisory"
url = "https://example.invalid/advisory"

[[advisory]]
id = "CIGI-2026-0002"
affects = [ "cig <1.7.0" ]
fixed_in = "1.7.0"
severity = "bug"
summary = "another example"
"#,
    );
    s
}

pub fn fleet() -> Manifest {
    Manifest::from_str(&fleet_text()).expect("the fleet fixture parses")
}
