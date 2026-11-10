// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `cargo xtask release prune` (#26): the keep rule applied to a product's GitHub releases.
//!
//! It reads, it plans, it prints. It never deletes, never pushes a tag, never uploads. The
//! plan is the dry run: every version of every product with keep or archive and the reason.
//! Running it for real (`--yes`) is refused until the archive index can be written and signed;
//! the issue's own words are "write and sign the index".

use std::collections::BTreeMap;

use cig_compat::{prune_plan, Manifest, PlanRow, ToolDef, Version};

use crate::assemble::{version_from_tag, Releases};

/// What `--yes` says until the index is signed.
pub const YES_BLOCKED: &str = "--yes is blocked: the archive index must be written and signed \
(#26: \"write and sign the index\", with the release key and the update channel's metadata, #9) \
before anything is archived or deleted. This tool only prints the plan.";

pub const HELP: &str = "\
usage: cargo xtask release prune [--keep 3] [--product <tool>] [--manifest <manifest.toml>]
                                 [--pin <tool>=<version>:<who>]... [--dry-run]

Plans the cleanup of #26 and prints it: every release version of each product, with keep or
archive and the reason. A product keeps its --keep latest versions (default 3) and every version
that is pinned: named by a carton of --manifest, or given with --pin (a Cargo tag, a digest
table, a toolchain pin). The plan is a dry run and always is.

It never deletes a release or a tag, and never uploads anything. `--yes` is blocked: archiving
needs the archive index written and signed first (#26: \"write and sign the index\").";

/// What the plan is built from.
#[derive(Clone, Debug)]
pub struct Options {
    pub keep: usize,
    /// Only this product, or every one of `manifest/tools.toml`.
    pub product: Option<String>,
    /// `(product, version)` -> who pins it.
    pub pins: BTreeMap<(String, Version), Vec<String>>,
}

/// One product's rows, newest first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProductPlan {
    pub product: String,
    pub repo: String,
    pub rows: Vec<PlanRow>,
}

/// The tool definitions of `manifest/tools.toml`, read by the client's own parser.
pub fn tools_of(tools_toml: &str) -> Result<BTreeMap<String, ToolDef>, String> {
    let text = format!(
        "format = 1\nchannel = \"stable\"\nsequence = 0\ngenerated = 2026-01-01T00:00:00Z\n\
         expires = 2026-02-01T00:00:00Z\n\n{tools_toml}\n"
    );
    Manifest::from_str(&text)
        .map(|m| m.tools)
        .map_err(|e| format!("manifest/tools.toml: {e}"))
}

/// Every build a carton of `m` includes: those versions are supported, so they are pinned.
pub fn pins_from_manifest(m: &Manifest) -> BTreeMap<(String, Version), Vec<String>> {
    let mut pins: BTreeMap<(String, Version), Vec<String>> = BTreeMap::new();
    for c in m.cartons.values() {
        for (tool, v) in &c.products {
            pins.entry((tool.clone(), v.clone()))
                .or_default()
                .push(format!("carton {}", c.version));
        }
    }
    pins
}

/// `cig=1.15.0:cig-compile 0.2.0 Cargo tag`.
pub fn parse_pin(arg: &str) -> Result<((String, Version), String), String> {
    let bad = || format!("--pin takes <tool>=<version>:<who>, not `{arg}`");
    let (tool, rest) = arg.split_once('=').ok_or_else(bad)?;
    let (ver, who) = rest.split_once(':').ok_or_else(bad)?;
    let version = Version::parse(ver).map_err(|e| format!("--pin `{arg}`: {e}"))?;
    if tool.is_empty() || who.trim().is_empty() {
        return Err(bad());
    }
    Ok(((tool.to_string(), version), who.trim().to_string()))
}

/// Plan every selected product. A product whose repository has no releases has an empty plan.
pub fn plan(
    rel: &dyn Releases,
    tools: &BTreeMap<String, ToolDef>,
    opts: &Options,
) -> Result<Vec<ProductPlan>, String> {
    if opts.keep == 0 {
        return Err("--keep must be at least 1".into());
    }
    if let Some(p) = &opts.product {
        if !tools.contains_key(p) {
            return Err(format!("`{p}` is not a product of manifest/tools.toml"));
        }
    }
    let mut out = Vec::new();
    for (name, def) in tools {
        if opts.product.as_ref().is_some_and(|p| p != name) {
            continue;
        }
        let tags = rel.tags(&def.repo)?;
        let versions: Vec<Version> = tags
            .iter()
            .filter_map(|t| version_from_tag(&def.tag, &t.tag))
            .collect();
        let pins: BTreeMap<Version, Vec<String>> = opts
            .pins
            .iter()
            .filter(|((t, _), _)| t == name)
            .map(|((_, v), who)| (v.clone(), who.clone()))
            .collect();
        out.push(ProductPlan {
            product: name.clone(),
            repo: def.repo.clone(),
            rows: prune_plan(&versions, opts.keep, &pins),
        });
    }
    Ok(out)
}

/// The dry run as text: one block per product, one line per version.
pub fn render(plans: &[ProductPlan], keep: usize) -> String {
    let mut out = String::new();
    for p in plans {
        let kept = p.rows.iter().filter(|r| r.keep).count();
        out.push_str(&format!(
            "{} ({}): {} versions, {} keep, {} archive\n",
            p.product,
            p.repo,
            p.rows.len(),
            kept,
            p.rows.len() - kept
        ));
        for r in &p.rows {
            out.push_str(&format!(
                "  {:<8} {:<14} {}\n",
                if r.keep { "keep" } else { "archive" },
                r.version,
                r.reason
            ));
        }
    }
    out.push_str(&format!(
        "dry run (keep {keep}): nothing was archived, deleted or uploaded\n"
    ));
    out
}
