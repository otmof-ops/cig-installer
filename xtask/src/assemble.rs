// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0
//! `cargo xtask manifest assemble` (SPEC 6.4): the next manifest of a channel.
//!
//! Three inputs. The tool definitions and the product requirements are files in `manifest/`,
//! changed by pull request like any code. The previous manifest is carried forward whole:
//! cartons are never edited or removed, and builds already published are never fetched again.
//! Each new build comes from the product's own published release. Its digest is the release's
//! `.sha256` sidecar, which must agree with the digest GitHub computed for the asset. Its size is
//! the release's.
//!
//! The document goes through the client's own parser, then its own renderer, then the parser
//! again. What this writes is what every installer will read.

use std::collections::BTreeMap;

use cig_compat::{Manifest, ToolDef, Version};
use sha2::{Digest, Sha256};
use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime};
use toml::{Table, Value};

/// One asset of a release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetInfo {
    pub id: u64,
    pub size: u64,
    /// GitHub's own sha256 of the asset, lowercase hex, when it reports one.
    pub digest: Option<String>,
}

/// A published release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseInfo {
    /// `YYYY-MM-DD`.
    pub published: String,
    pub prerelease: bool,
    pub assets: BTreeMap<String, AssetInfo>,
}

/// Where releases are read from: GitHub through the maintainer's `gh` ([`crate::gh::Gh`]), or a
/// fake in the tests.
pub trait Releases {
    /// The published release `tag` of `repo` (a draft is not published).
    fn release(&self, repo: &str, tag: &str) -> Result<ReleaseInfo, String>;
    /// The bytes of a small asset (a sidecar), refused above `max` bytes.
    fn small_asset(&self, repo: &str, asset: &AssetInfo, max: u64) -> Result<Vec<u8>, String>;
}

/// What to assemble.
#[derive(Clone, Debug)]
pub struct Request {
    pub channel: String,
    pub carton: Version,
    /// Each product of the carton and its exact version.
    pub members: BTreeMap<String, Version>,
    pub packs: BTreeMap<String, Vec<String>>,
    pub notes: String,
    /// Days from now until the manifest expires (clients refuse it after that).
    pub expires_days: i64,
    /// The text of the channel's current manifest; `None` for the first one.
    pub previous: Option<String>,
    /// `manifest/tools.toml`.
    pub tools_toml: String,
    /// `manifest/products.toml`.
    pub products_toml: String,
    pub now: OffsetDateTime,
}

/// The result: the document, its digest, and what was done.
#[derive(Debug)]
pub struct Assembled {
    pub manifest: Manifest,
    /// The document as written.
    pub text: String,
    /// sha256 of `text`, lowercase hex: the `.sha256` sidecar's digest.
    pub sha256: String,
    /// One line per thing done, for the maintainer to read before uploading.
    pub report: Vec<String>,
}

/// Clients refuse a manifest that expires more than 90 days ahead (SPEC 7.2).
pub const MAX_EXPIRES_DAYS: i64 = 90;
/// A sidecar is one line; anything bigger is not a sidecar.
const MAX_SIDECAR: u64 = 4096;
/// The parser refuses sizes of 1 GB and over.
const MAX_ASSET: u64 = 1 << 30;

fn timestamp(t: OffsetDateTime) -> Result<String, String> {
    t.format(&Rfc3339).map_err(|e| e.to_string())
}

fn datetime(text: &str) -> Result<Value, String> {
    text.parse::<toml::value::Datetime>()
        .map(Value::Datetime)
        .map_err(|e| format!("`{text}` is not a date: {e}"))
}

/// The table at `keys` under `t`, made when missing; an error when something else is there.
fn table_at<'a>(t: &'a mut Table, keys: &[&str]) -> Result<&'a mut Table, String> {
    let mut cur = t;
    for k in keys {
        let v = cur
            .entry((*k).to_string())
            .or_insert_with(|| Value::Table(Table::new()));
        cur = match v {
            Value::Table(t) => t,
            _ => return Err(format!("`{}` is not a table", keys.join("."))),
        };
    }
    Ok(cur)
}

/// Parse `manifest/tools.toml` and `manifest/products.toml` with the client's parser, by giving
/// them the header a manifest needs.
fn definitions(req: &Request, generated: &str, expires: &str) -> Result<Manifest, String> {
    let text = format!(
        "format = 1\nchannel = \"{}\"\nsequence = 0\ngenerated = {generated}\nexpires = {expires}\n\n{}\n\n{}\n",
        req.channel, req.tools_toml, req.products_toml
    );
    Manifest::from_str(&text)
        .map_err(|e| format!("manifest/tools.toml with manifest/products.toml: {e}"))
}

/// Whether a published build of `old` is still fetched the same way under `new`. The targets and
/// `attests` may change; the names a published build has may not.
fn same_fetch(old: &ToolDef, new: &ToolDef) -> bool {
    old.repo == new.repo
        && old.tag == new.tag
        && old.layout == new.layout
        && old.asset == new.asset
        && old.sidecar == new.sidecar
        && old.strip == new.strip
        && old.shims == new.shims
}

/// Every `requires` of every product of the carton holds inside the carton.
fn requires_hold(m: &Manifest, carton: &Version) -> Result<(), String> {
    let c = m
        .carton(carton)
        .ok_or_else(|| format!("carton {carton} is missing from what was assembled"))?;
    for (tool, v) in &c.products {
        let Some(p) = m.product(tool, v) else {
            continue;
        };
        for (need, r) in &p.requires {
            match c.products.get(need) {
                Some(have) if r.matches(have) => {}
                Some(have) => {
                    return Err(format!(
                        "{tool} {v} needs {need} {}, and carton {carton} has {need} {have}",
                        r.text
                    ))
                }
                None => {
                    return Err(format!(
                        "{tool} {v} needs {need} {}, and carton {carton} has no {need}",
                        r.text
                    ))
                }
            }
        }
    }
    Ok(())
}

/// The `[build.<tool>."<v>"]` table of a build not published before.
fn fetch_build(
    tool: &str,
    v: &Version,
    def: &ToolDef,
    channel: &str,
    rel: &dyn Releases,
    report: &mut Vec<String>,
) -> Result<Table, String> {
    let first = def
        .targets
        .first()
        .ok_or_else(|| format!("tool.{tool} has no targets"))?;
    let tag = def
        .names(v, first)
        .map_err(|e| format!("tool.{tool}: {e}"))?
        .tag;
    let release = rel.release(&def.repo, &tag)?;
    if release.prerelease && channel == "stable" {
        return Err(format!(
            "{} {tag} is a prerelease, and the stable channel takes none",
            def.repo
        ));
    }
    let mut t = Table::new();
    t.insert("released".into(), datetime(&release.published)?);
    let (mut found, mut missing) = (Vec::new(), Vec::new());
    for target in &def.targets {
        let n = def
            .names(v, target)
            .map_err(|e| format!("tool.{tool}: {e}"))?;
        let Some(asset) = release.assets.get(&n.asset) else {
            missing.push(target.clone());
            continue;
        };
        let side = release.assets.get(&n.sidecar).ok_or_else(|| {
            format!(
                "{} {tag}: {} has no sidecar {}; every asset needs one",
                def.repo, n.asset, n.sidecar
            )
        })?;
        let bytes = rel.small_asset(&def.repo, side, MAX_SIDECAR)?;
        let text = String::from_utf8(bytes).map_err(|_| format!("{} is not text", n.sidecar))?;
        let (digest, named) = cigi_source::sidecar::parse_named(&text)
            .map_err(|e| format!("{} {tag}: {}: {e}", def.repo, n.sidecar))?;
        if let Some(named) = named {
            if named != n.asset {
                return Err(format!("{} names {named}, not {}", n.sidecar, n.asset));
            }
        }
        match &asset.digest {
            Some(d) if *d != digest => {
                let what = format!("{} {tag}: the sidecar of {}", def.repo, n.asset);
                return Err(format!("{what} says {digest}; GitHub's own digest is {d}"));
            }
            Some(_) => {}
            None => report.push(format!(
                "{}: GitHub reports no digest for it, so its sidecar alone vouches for it",
                n.asset
            )),
        }
        if asset.size == 0 || asset.size >= MAX_ASSET {
            return Err(format!("{} is {} bytes", n.asset, asset.size));
        }
        let mut a = Table::new();
        a.insert("sha256".into(), Value::String(digest));
        a.insert("size".into(), Value::Integer(asset.size as i64));
        t.insert(target.clone(), Value::Table(a));
        found.push(target.clone());
    }
    if found.is_empty() {
        return Err(format!(
            "{tool} {v}: no target of tool.{tool} has an asset on {} {tag}",
            def.repo
        ));
    }
    report.push(format!(
        "{tool} {v}: {} from {} {tag}; each sidecar agrees with GitHub's digest",
        found.join(", "),
        def.repo
    ));
    if !missing.is_empty() {
        report.push(format!(
            "{tool} {v}: no build for {}, so this carton cannot be installed there",
            missing.join(", ")
        ));
    }
    Ok(t)
}

/// Assemble the next manifest (module docs).
pub fn assemble(req: &Request, rel: &dyn Releases) -> Result<Assembled, String> {
    if !(1..=MAX_EXPIRES_DAYS).contains(&req.expires_days) {
        return Err(format!(
            "--expires-days is 1 to {MAX_EXPIRES_DAYS}: clients refuse a manifest that expires later"
        ));
    }
    if req.members.is_empty() {
        return Err("a carton holds at least one product".into());
    }
    let now = req.now.replace_nanosecond(0).unwrap_or(req.now);
    let generated = timestamp(now)?;
    let expires = timestamp(now + Duration::days(req.expires_days))?;
    let today = generated[..10].to_string();
    let defs = definitions(req, &generated, &expires)?;
    let tools_doc: Table = req
        .tools_toml
        .parse()
        .map_err(|e| format!("manifest/tools.toml: {e}"))?;
    let products_doc: Table = req
        .products_toml
        .parse()
        .map_err(|e| format!("manifest/products.toml: {e}"))?;

    let mut report = Vec::new();
    let previous = match &req.previous {
        Some(t) => Some(Manifest::from_str(t).map_err(|e| format!("the previous manifest: {e}"))?),
        None => None,
    };
    let mut doc: Table = match &req.previous {
        Some(t) => t
            .parse()
            .map_err(|e| format!("the previous manifest: {e}"))?,
        None => Table::new(),
    };
    let sequence = match &previous {
        Some(p) => {
            if p.channel != req.channel {
                return Err(format!(
                    "the previous manifest is the {} channel's, not the {} channel's",
                    p.channel, req.channel
                ));
            }
            if p.cartons.contains_key(&req.carton) {
                return Err(format!(
                    "carton {} is already published; a carton is never edited, so give the change a new number",
                    req.carton
                ));
            }
            p.sequence + 1
        }
        None => 1,
    };
    doc.insert("format".into(), Value::Integer(1));
    doc.insert("channel".into(), Value::String(req.channel.clone()));
    doc.insert("sequence".into(), Value::Integer(sequence as i64));
    doc.insert("generated".into(), datetime(&generated)?);
    doc.insert("expires".into(), datetime(&expires)?);

    // the definitions: manifest/tools.toml's, except that a published build is fetched as before
    if let Some(p) = &previous {
        for (name, old) in &p.tools {
            let published = p.builds.keys().any(|(t, _)| t == name);
            match defs.tools.get(name) {
                Some(new) if published && !same_fetch(old, new) => {
                    return Err(format!(
                        "manifest/tools.toml changes how tool.{name}'s published builds are fetched \
                         (repo, tag, layout, asset, sidecar, strip or shims); those names never change"
                    ))
                }
                Some(_) => {}
                None => report.push(format!(
                    "tool.{name} is kept from the previous manifest (manifest/tools.toml lacks it)"
                )),
            }
        }
    }
    if let Some(Value::Table(tools)) = tools_doc.get("tool") {
        let into = table_at(&mut doc, &["tool"])?;
        for (name, t) in tools {
            into.insert(name.clone(), t.clone());
        }
    }

    for (tool, v) in &req.members {
        let def = defs
            .tools
            .get(tool)
            .ok_or_else(|| format!("{tool} is not defined in manifest/tools.toml"))?;
        if previous
            .as_ref()
            .is_some_and(|p| p.is_product_yanked(tool, v))
        {
            return Err(format!("{tool} {v} is yanked in the previous manifest"));
        }
        let vs = v.to_string();
        let product = products_doc
            .get("product")
            .and_then(|p| p.get(tool))
            .and_then(|p| p.get(&vs))
            .and_then(Value::as_table)
            .ok_or_else(|| {
                format!(
                    "manifest/products.toml has no [product.{tool}.\"{vs}\"]; write one, even if it is only `requires = {{}}`"
                )
            })?;
        let into = table_at(&mut doc, &["product", tool])?;
        if let Some(old) = into.get(&vs) {
            if old.as_table() != Some(product) {
                report.push(format!(
                    "[product.{tool}.\"{vs}\"] differs from the previous manifest's; this one replaces it"
                ));
            }
        }
        into.insert(vs.clone(), Value::Table(product.clone()));

        let published = previous
            .as_ref()
            .is_some_and(|p| p.builds.contains_key(&(tool.clone(), v.clone())));
        if published {
            report.push(format!(
                "{tool} {v}: published before, so its build tables are carried forward"
            ));
            continue;
        }
        let build = fetch_build(tool, v, def, &req.channel, rel, &mut report)?;
        table_at(&mut doc, &["build", tool])?.insert(vs, Value::Table(build));
    }

    let mut c = Table::new();
    c.insert("released".into(), datetime(&today)?);
    if !req.notes.is_empty() {
        c.insert("notes".into(), Value::String(req.notes.clone()));
    }
    c.insert("tested".into(), Value::Array(Vec::new()));
    for (tool, v) in &req.members {
        c.insert(tool.clone(), Value::String(v.to_string()));
    }
    if !req.packs.is_empty() {
        let mut packs = Table::new();
        for (name, tools) in &req.packs {
            packs.insert(
                name.clone(),
                Value::Array(tools.iter().cloned().map(Value::String).collect()),
            );
        }
        c.insert("packs".into(), Value::Table(packs));
    }
    table_at(&mut doc, &["carton"])?.insert(req.carton.to_string(), Value::Table(c));

    let first = toml::to_string(&doc).map_err(|e| e.to_string())?;
    let manifest = Manifest::from_str(&first)
        .map_err(|e| format!("what was assembled does not parse: {e}"))?;
    requires_hold(&manifest, &req.carton)?;
    let text = manifest.render();
    let again = Manifest::from_str(&text)
        .map_err(|e| format!("the rendered manifest does not parse (a bug): {e}"))?;
    if again != manifest {
        return Err("the rendered manifest does not read back the same (a bug)".into());
    }
    report.push(format!(
        "carton {} on the {} channel, sequence {sequence}, expires {expires}",
        req.carton, req.channel
    ));
    report
        .push("tested = []: no integration suite has run against exactly this set yet (#3)".into());
    let sha256 = hex::encode(Sha256::digest(text.as_bytes()));
    Ok(Assembled {
        manifest,
        text,
        sha256,
        report,
    })
}

/// The `.sha256` sidecar of a manifest file, as `sha256sum` writes it.
pub fn sidecar_line(sha256: &str, file_name: &str) -> String {
    format!("{sha256}  {file_name}\n")
}

#[cfg(test)]
mod tests;
