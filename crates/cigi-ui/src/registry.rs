// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! The error registry, read from `errors/registry.toml` (SPEC section 11.3), in the schema
//! of cig's registry (`errors::{Range, No, Probe, Cause, Code}` in cigscript) so cig-style
//! `explain` works. The file is embedded; a malformed one is a build defect caught by the
//! tests, and reads as an empty registry here rather than a panic.

use crate::error::Kind;
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
