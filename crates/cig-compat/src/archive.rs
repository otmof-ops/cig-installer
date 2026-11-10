// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! The archive index (#26): where every archived version of a product went, and the keep rule
//! that decides what is archived. Pure: it reads text handed to it. Fetching and checking the
//! index's signature belong to the update channel (#9); a parsed index is only as good as the
//! bytes it was parsed from.
//!
//! `archive/index.json`:
//!
//! ```json
//! {"format": 1, "entries": [
//!   {"product": "cig", "version": "1.6.2",
//!    "tag": {"repo": "otmof-ops/cig-script-archive", "ref": "cig/v1.6.2", "commit": "3f9c..."},
//!    "assets": [{"name": "cig-v1.6.2-linux-x86_64", "url": "https://github.com/...", "sha256": "..."}],
//!    "archived": "2026-11-12", "reason": "older than the three latest"}]}
//! ```

use crate::manifest::{ManifestError, E_MALFORMED};
use crate::versions::Version;
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// The index format this reads.
pub const INDEX_FORMAT: u64 = 1;
const MAX_TEXT: usize = 16 * 1024 * 1024;
const URL_PREFIX: &str = "https://github.com/";

fn err<T>(message: impl Into<String>) -> Result<T, ManifestError> {
    Err(ManifestError {
        code: E_MALFORMED,
        message: format!("archive index: {}", message.into()),
    })
}

/// `[archive]` of a manifest: where the signed index is published.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveRef {
    /// An `https://` URL of `archive/index.json`.
    pub index: String,
}

impl ArchiveRef {
    pub fn parse(index: &str) -> Result<ArchiveRef, ManifestError> {
        if !index.starts_with(URL_PREFIX)
            || index.len() > 512
            || index.chars().any(|c| c.is_whitespace() || c.is_control())
        {
            return err(format!("`{index}` is not a {URL_PREFIX} URL"));
        }
        Ok(ArchiveRef {
            index: index.to_string(),
        })
    }
}

/// Where an archived version's tag lives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveTag {
    pub repo: String,
    /// `cig/v1.6.2`: the archive repository's ref for it.
    pub reference: String,
    pub commit: String,
}

/// One re-uploaded release asset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveAsset {
    pub name: String,
    pub url: String,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveEntry {
    pub product: String,
    pub version: Version,
    pub tag: ArchiveTag,
    pub assets: Vec<ArchiveAsset>,
    pub archived: String,
    pub reason: String,
}

impl ArchiveEntry {
    /// The archived asset called `name`.
    pub fn asset(&self, name: &str) -> Option<&ArchiveAsset> {
        self.assets.iter().find(|a| a.name == name)
    }
}

/// Every archived version, one entry per `(product, version)`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ArchiveIndex {
    pub entries: Vec<ArchiveEntry>,
}

fn plain(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('.')
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'+' | b'-'))
}

fn hex_ok(s: &str, lens: &[usize]) -> bool {
    lens.contains(&s.len()) && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn text<'a>(v: &'a Value, key: &str, path: &str) -> Result<&'a str, ManifestError> {
    match v.get(key).and_then(Value::as_str) {
        Some(s) if !s.is_empty() => Ok(s),
        _ => err(format!("{path}.{key} must be a non-empty string")),
    }
}

fn parse_entry(v: &Value, path: &str) -> Result<ArchiveEntry, ManifestError> {
    let product = text(v, "product", path)?;
    if !plain(product) {
        return err(format!("{path}.product `{product}` is not a plain name"));
    }
    let version = Version::parse(text(v, "version", path)?)
        .or_else(|e| err(format!("{path}.version: {e}")))?;
    let tag = v
        .get("tag")
        .filter(|t| t.is_object())
        .map_or_else(|| err(format!("{path}.tag is missing")), Ok)?;
    let tp = format!("{path}.tag");
    let repo = text(tag, "repo", &tp)?;
    let reference = text(tag, "ref", &tp)?;
    let commit = text(tag, "commit", &tp)?;
    let repo_ok = repo
        .split_once('/')
        .is_some_and(|(a, b)| plain(a) && plain(b));
    if !repo_ok {
        return err(format!("{tp}.repo `{repo}` is not owner/name"));
    }
    if reference.split('/').any(|s| !plain(s)) {
        return err(format!("{tp}.ref `{reference}` is not a plain ref"));
    }
    if !hex_ok(commit, &[40, 64]) {
        return err(format!("{tp}.commit is not a full lowercase hex commit id"));
    }
    let Some(list) = v.get("assets").and_then(Value::as_array) else {
        return err(format!("{path}.assets must be an array"));
    };
    let mut assets = Vec::new();
    for (i, a) in list.iter().enumerate() {
        let ap = format!("{path}.assets[{i}]");
        let name = text(a, "name", &ap)?;
        let url = text(a, "url", &ap)?;
        let sha256 = text(a, "sha256", &ap)?;
        if !plain(name) {
            return err(format!("{ap}.name `{name}` is not a plain file name"));
        }
        if ArchiveRef::parse(url).is_err() || url.rsplit('/').next() != Some(name) {
            return err(format!(
                "{ap}.url must be a {URL_PREFIX} URL that ends in the asset's name"
            ));
        }
        if !hex_ok(sha256, &[64]) {
            return err(format!("{ap}.sha256 must be 64 lowercase hex digits"));
        }
        if assets.iter().any(|x: &ArchiveAsset| x.name == name) {
            return err(format!("{ap}: `{name}` is listed twice"));
        }
        assets.push(ArchiveAsset {
            name: name.to_string(),
            url: url.to_string(),
            sha256: sha256.to_string(),
        });
    }
    Ok(ArchiveEntry {
        product: product.to_string(),
        version,
        tag: ArchiveTag {
            repo: repo.to_string(),
            reference: reference.to_string(),
            commit: commit.to_string(),
        },
        assets,
        archived: text(v, "archived", path)?.to_string(),
        reason: text(v, "reason", path)?.to_string(),
    })
}

impl ArchiveIndex {
    /// Parse and validate `archive/index.json`. Every refusal is `E1825`.
    pub fn from_json(body: &str) -> Result<ArchiveIndex, ManifestError> {
        if body.len() > MAX_TEXT {
            return err("larger than 16 MiB");
        }
        let root: Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(e) => return err(format!("not valid JSON: {e}")),
        };
        match root.get("format").and_then(Value::as_u64) {
            Some(INDEX_FORMAT) => {}
            Some(n) if n > INDEX_FORMAT => {
                return err(format!("format {n} is newer than this ciginstaller reads"))
            }
            _ => return err("`format` must be 1"),
        }
        let Some(list) = root.get("entries").and_then(Value::as_array) else {
            return err("`entries` must be an array");
        };
        let mut index = ArchiveIndex::default();
        for (i, e) in list.iter().enumerate() {
            let entry = parse_entry(e, &format!("entries[{i}]"))?;
            if index.find(&entry.product, &entry.version).is_some() {
                return err(format!(
                    "entries[{i}]: {} {} is listed twice",
                    entry.product, entry.version
                ));
            }
            index.entries.push(entry);
        }
        Ok(index)
    }

    /// The index as JSON, in version order; `from_json(to_json(i)) == i`.
    pub fn to_json(&self) -> String {
        let mut sorted: Vec<&ArchiveEntry> = self.entries.iter().collect();
        sorted.sort_by(|a, b| (&a.product, &a.version).cmp(&(&b.product, &b.version)));
        let entries: Vec<Value> = sorted
            .into_iter()
            .map(|e| {
                json!({
                    "product": e.product,
                    "version": e.version.to_string(),
                    "tag": {"repo": e.tag.repo, "ref": e.tag.reference, "commit": e.tag.commit},
                    "assets": e.assets.iter().map(|a| json!({
                        "name": a.name, "url": a.url, "sha256": a.sha256
                    })).collect::<Vec<_>>(),
                    "archived": e.archived,
                    "reason": e.reason,
                })
            })
            .collect();
        serde_json::to_string_pretty(&json!({"format": INDEX_FORMAT, "entries": entries}))
            .unwrap_or_default()
    }

    pub fn find(&self, product: &str, version: &Version) -> Option<&ArchiveEntry> {
        self.entries
            .iter()
            .find(|e| e.product == product && &e.version == version)
    }

    /// Add an entry, refusing a version something still pins. This is the second place a pinned
    /// tag is protected: the plan never archives it, and the index never lists it.
    pub fn add(
        &mut self,
        entry: ArchiveEntry,
        pins: &BTreeMap<(String, Version), Vec<String>>,
    ) -> Result<(), String> {
        if let Some(who) = pins.get(&(entry.product.clone(), entry.version.clone())) {
            return Err(format!(
                "{} {} is pinned by {} and is never archived",
                entry.product,
                entry.version,
                who.join(", ")
            ));
        }
        if self.find(&entry.product, &entry.version).is_some() {
            return Err(format!(
                "{} {} is already archived",
                entry.product, entry.version
            ));
        }
        self.entries.push(entry);
        Ok(())
    }
}

// ---------------------------------------------------------------- the keep rule

/// One version's line in a prune plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanRow {
    pub version: Version,
    /// `true`: it stays on the product's repository.
    pub keep: bool,
    pub reason: String,
}

/// The keep rule of #26: the `keep_latest` newest versions stay, and so does every version in
/// `pins` (value: who pins it). Everything else is archived. Rows come newest first; every
/// version gets one row and a reason. Prereleases rank with the rest: they are releases too.
pub fn prune_plan(
    versions: &[Version],
    keep_latest: usize,
    pins: &BTreeMap<Version, Vec<String>>,
) -> Vec<PlanRow> {
    let mut all: Vec<&Version> = versions.iter().collect();
    all.sort_by(|a, b| b.cmp(a));
    all.dedup();
    all.into_iter()
        .enumerate()
        .map(|(rank, v)| {
            let latest = rank < keep_latest;
            let pinned = pins.get(v).filter(|w| !w.is_empty());
            let mut parts = Vec::new();
            if latest {
                parts.push(format!("one of the {keep_latest} latest"));
            }
            if let Some(who) = pinned {
                parts.push(format!("pinned by {}", who.join(", ")));
            }
            let keep = latest || pinned.is_some();
            if !keep {
                parts.push(format!(
                    "older than the {keep_latest} latest and not pinned"
                ));
            }
            PlanRow {
                version: v.clone(),
                keep,
                reason: parts.join("; "),
            }
        })
        .collect()
}
