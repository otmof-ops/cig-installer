// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! The error registry, read from `errors/registry.toml` (SPEC section 11.3), in the schema
//! of cig's registry (`errors::{Range, No, Probe, Cause, Code}` in cigscript) so cig-style
//! `explain` works. The file is embedded; a malformed one is a build defect caught by the
//! tests, and reads as an empty registry here rather than a panic.

use crate::catalogue;
use crate::error::Kind;
use crate::tokens::Role;
use serde::Deserialize;
use std::sync::OnceLock;

/// The registry source.
pub const REGISTRY_TOML: &str = include_str!("../../../errors/registry.toml");

#[derive(Debug, Default, Deserialize)]
pub struct Registry {
    pub version: u32,
    #[serde(rename = "range", default)]
    pub ranges: Vec<Range>,
    #[serde(rename = "no", default)]
    pub nos: Vec<No>,
    #[serde(rename = "probe", default)]
    pub probes: Vec<Probe>,
    #[serde(rename = "code", default)]
    pub codes: Vec<Code>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Range {
    pub from: String,
    pub to: String,
    pub family: String,
    pub kind: Kind,
    pub meaning: String,
}

/// One of the four kinds of no (plus `none` and `depends`).
#[derive(Debug, Clone, Deserialize)]
pub struct No {
    pub id: String,
    pub hear: String,
    pub whose: String,
    pub manual: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Probe {
    pub name: String,
    pub reads: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Cause {
    pub why: String,
    #[serde(default)]
    pub no: Option<String>,
    #[serde(default = "none")]
    pub probe: String,
    pub remedy: String,
    #[serde(default)]
    pub fixable: bool,
    #[serde(default)]
    pub needs_burn: bool,
    #[serde(default = "yes")]
    pub reversible: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Code {
    pub code: String,
    pub title: String,
    pub meaning: String,
    /// What to type next.
    pub fix: String,
    #[serde(default = "none")]
    pub no: String,
    #[serde(default)]
    pub arises: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub since: String,
    #[serde(default)]
    pub retired: bool,
    #[serde(default)]
    pub related: Vec<String>,
    #[serde(default, rename = "cause")]
    pub causes: Vec<Cause>,
}

fn none() -> String {
    "none".to_string()
}

fn yes() -> bool {
    true
}

/// A code's number: `E1811` is 1811. Compared by number, never as text.
pub fn number(code: &str) -> Option<u32> {
    let digits = code.strip_prefix('E')?;
    if !(3..=4).contains(&digits.len()) || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

impl Range {
    pub fn holds(&self, n: u32) -> bool {
        number(&self.from).is_some_and(|f| f <= n) && number(&self.to).is_some_and(|t| n <= t)
    }
}

impl Code {
    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|t| t == tag)
    }

    pub fn range(&self) -> Option<&'static Range> {
        let n = number(&self.code)?;
        registry().ranges.iter().find(|r| r.holds(n))
    }

    pub fn kind(&self) -> Option<Kind> {
        self.range().map(|r| r.kind)
    }

    pub fn family(&self) -> &'static str {
        self.range().map_or("installer", |r| r.family.as_str())
    }

    pub fn no_entry(&self) -> Option<&'static No> {
        lookup_no(&self.no)
    }
}

pub fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| toml::from_str(REGISTRY_TOML).unwrap_or_default())
}

pub fn all() -> &'static [Code] {
    &registry().codes
}

pub fn lookup(code: &str) -> Option<&'static Code> {
    let wanted = code.trim().to_ascii_uppercase();
    all().iter().find(|c| c.code == wanted)
}

pub fn lookup_no(id: &str) -> Option<&'static No> {
    registry().nos.iter().find(|n| n.id == id)
}

pub fn lookup_probe(name: &str) -> Option<&'static Probe> {
    registry().probes.iter().find(|p| p.name == name)
}

pub fn kind_of(code: &str) -> Option<Kind> {
    let n = number(code)?;
    registry()
        .ranges
        .iter()
        .find(|r| r.holds(n))
        .map(|r| r.kind)
}

/// The catchphrase of the kind of no a code is ("can't see any ciggies bro"), if it has one.
pub fn catchphrase(code: &str) -> Option<&'static str> {
    let no = lookup(code)?.no_entry()?;
    if matches!(no.id.as_str(), "none" | "depends") {
        None
    } else {
        Some(no.hear.as_str())
    }
}

/// The plain form of a code's kind of no ("not found"), written beside the voice in the
/// catalogue. `None` for `none` and `depends`.
pub fn plain_label(code: &str) -> Option<&'static str> {
    let no = lookup(code)?.no_entry()?;
    catalogue::kind_of_no(&no.id).map(|m| m.plain)
}

/// The role a code is painted in (#31, "four kinds of no"): `cant-see-any` and `dont-have-any`
/// are neutral notices (`info`), `these-are-mine` and `never-heard-of-you` are the door's
/// (`warn`), a bug in the installer is `danger`. A code with no kind of no (`none`) is a
/// mistake in us or in the arithmetic and is `danger`; `depends` reads the operating system's
/// reason and is `warn`. An unknown code is `danger`.
pub fn role_of(code: &str) -> Role {
    if kind_of(code) == Some(Kind::Internal) {
        return Role::Danger;
    }
    match lookup(code).map(|c| c.no.as_str()) {
        Some("cant-see-any" | "dont-have-any") => Role::Info,
        Some("these-are-mine" | "never-heard-of-you" | "depends") => Role::Warn,
        _ => Role::Danger,
    }
}

/// The area a code belongs to, the second word of `error[E1811 carton]`: what the code is
/// about, in one lowercase word. Ranges follow `errors/registry.toml`.
pub fn area(code: &str) -> &'static str {
    match number(code) {
        Some(1800..=1809) => "check",
        Some(1810) => "manifest",
        Some(1811 | 1816..=1818) => "carton",
        Some(1812) => "channel",
        Some(1813) => "pack",
        Some(1814) => "tool",
        Some(1815) => "platform",
        Some(1820..=1821) => "fetch",
        Some(1822..=1829) => "trust",
        Some(1830..=1836) => "store",
        Some(1840..=1841 | 1844..=1845) => "shim",
        Some(1842..=1843 | 1846..=1848) => "pin",
        Some(1850..=1856) => "update",
        Some(1860..=1862) => "repair",
        Some(1863 | 1865 | 1868) => "uninstall",
        Some(1869) => "path",
        Some(1864) => "doctor",
        Some(1870..=1873) => "usage",
        Some(1874) => "shim",
        Some(1875) => "config",
        Some(1890..=1899) => "internal",
        _ => "installer",
    }
}

/// The registry page for a code, as text. `plain` drops the catchphrases and uses the
/// manual's name for the kind of no.
pub fn explain_with(code: &str, plain: bool) -> Option<String> {
    let e = lookup(code)?;
    let mut out = String::new();
    let kind = e.kind().map_or("usage", Kind::as_str);
    out.push_str(&format!(
        "{} {}  ({}, {} family, since {})\n",
        e.code,
        e.title,
        kind,
        e.family(),
        e.since
    ));
    if e.has_tag("planned") {
        out.push_str(
            "  planned: another tool will report it once it calls the installer's check\n",
        );
    }
    out.push('\n');
    if let Some(no) = e.no_entry() {
        if plain {
            out.push_str(&format!("  kind of no: {}  ({})\n", no.manual, no.whose));
        } else {
            out.push_str(&format!(
                "  kind of no: {}  ({}; {})\n",
                no.hear, no.whose, no.manual
            ));
        }
    }
    out.push_str(&format!(
        "\n  {}\n\n  what to type next: {}\n",
        e.meaning, e.fix
    ));
    if !e.causes.is_empty() {
        out.push_str("\n  known causes, ranked; doctor checks them in this order\n");
    }
    for (i, c) in e.causes.iter().enumerate() {
        out.push_str(&format!("  {}. {}\n", i + 1, c.why));
        match lookup_probe(&c.probe) {
            Some(p) if p.name != "none" => {
                out.push_str(&format!("     probe: {} ({})\n", p.name, p.reads))
            }
            _ => out.push_str("     probe: none; a suggestion, not a finding\n"),
        }
        out.push_str(&format!("     remedy: {}\n", c.remedy));
    }
    if !e.related.is_empty() {
        out.push_str(&format!("\n  related: {}\n", e.related.join(", ")));
    }
    Some(out)
}

/// `explain_with(code, false)`.
pub fn explain(code: &str) -> Option<String> {
    explain_with(code, false)
}

/// Every code by range, one line each, for `ciginstaller explain` with no argument.
pub fn explain_list() -> String {
    let mut out = String::from(
        "ciginstaller error codes\na code never changes meaning; a retired code is never reused\n",
    );
    for r in &registry().ranges {
        out.push_str(&format!("\n{}-{}  {}\n", r.from, r.to, r.meaning));
        for c in all()
            .iter()
            .filter(|c| number(&c.code).is_some_and(|n| r.holds(n)))
        {
            let mark = if c.has_tag("planned") {
                "  (planned)"
            } else {
                ""
            };
            out.push_str(&format!("  {}  {:<44}{mark}\n", c.code, c.title));
        }
    }
    out.push_str("\nciginstaller explain <code> for the page\n");
    out
}
