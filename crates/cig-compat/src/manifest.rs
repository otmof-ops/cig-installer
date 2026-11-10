// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! The carton manifest (section 6 of the build contract): parse, validate,
//! render, query, diff. The only I/O-free entry point is [`Manifest::from_str`].

use crate::archive::ArchiveRef;
use crate::time::rfc3339_to_epoch;
use crate::versions::{CartonVersion, Pin, Version};
use std::collections::BTreeMap;
use std::fmt;
use toml::{Table, Value};

/// Error code for a manifest that is malformed or too new.
pub const E_MALFORMED: &str = "E1825";
const MAX_TEXT: usize = 16 * 1024 * 1024;
const MAX_SIZE: u64 = 1_000_000_000;
const RESERVED_CARTON_KEYS: [&str; 5] = ["released", "notes", "tested", "packs", "advisory"];

/// Why a manifest was refused. `code` is always `E1825`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManifestError {
    pub code: &'static str,
    pub message: String,
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for ManifestError {}

fn err<T>(message: impl Into<String>) -> Result<T, ManifestError> {
    Err(ManifestError {
        code: E_MALFORMED,
        message: message.into(),
    })
}

/// An operating system and architecture, as `std::env::consts` spells them.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Target {
    pub os: String,
    pub arch: String,
}

impl Target {
    pub fn host() -> Target {
        Target {
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
        }
    }

    /// `linux-x86_64`.
    pub fn parse(s: &str) -> Option<Target> {
        let (os, arch) = s.split_once('-')?;
        if os.is_empty() || arch.is_empty() {
            return None;
        }
        Some(Target {
            os: os.to_string(),
            arch: arch.to_string(),
        })
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.os, self.arch)
    }
}

/// How a product is packaged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// One file per target.
    Binary,
    /// One `.tar.gz` for the target (or for every target when it is `any`).
    Archive,
}

impl Layout {
    pub fn as_str(self) -> &'static str {
        match self {
            Layout::Binary => "binary",
            Layout::Archive => "archive",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShimDef {
    pub name: String,
    pub path: String,
    pub windows_path: Option<String>,
}

/// `[tool.<name>]`: how one product is distributed.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolDef {
    pub repo: String,
    pub tag: String,
    pub layout: Layout,
    pub asset: String,
    pub sidecar: String,
    pub strip: u32,
    pub shims: Vec<ShimDef>,
    pub targets: Vec<String>,
    pub attests: bool,
    pub extra: Table,
}

/// What one build of a tool is called on its release: the tag, the asset and its sidecar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseNames {
    pub tag: String,
    pub asset: String,
    pub sidecar: String,
}

impl ToolDef {
    /// The names `version` has for `target` (`any`, or `os-arch`), from this tool's templates.
    /// The one place the templates are filled in: the parser and `xtask manifest assemble` agree.
    pub fn names(&self, version: &Version, target: &str) -> Result<ReleaseNames, String> {
        let ver = version.to_string();
        let mut vars: Vec<(&str, &str)> = vec![("version", &ver)];
        let (os, arch);
        if target != "any" {
            let Some(tg) = Target::parse(target) else {
                return Err(format!("`{target}` is not a target"));
            };
            os = tg.os;
            arch = tg.arch;
            vars.push(("os", &os));
            vars.push(("arch", &arch));
            vars.push(("exe", if os == "windows" { ".exe" } else { "" }));
        }
        let tag = expand(&self.tag, &vars)?;
        let asset = expand(&self.asset, &vars)?;
        vars.push(("asset", &asset));
        let sidecar = expand(&self.sidecar, &vars)?;
        Ok(ReleaseNames {
            tag,
            asset,
            sidecar,
        })
    }
}

/// One downloadable file of one build for one target.
#[derive(Clone, Debug, PartialEq)]
pub struct Artifact {
    pub tool: String,
    pub version: Version,
    /// `linux-x86_64`, or `any`.
    pub target: String,
    pub sha256: String,
    pub size: u64,
    /// As written in the manifest; `None` means the default release URL.
    pub url: Option<String>,
    pub repo: String,
    pub tag: String,
    pub layout: Layout,
    pub strip: u32,
    pub attests: bool,
    /// The asset's file name, from the tool's template.
    pub asset: String,
    /// The sidecar's file name.
    pub sidecar: String,
    /// Where to fetch the asset (the manifest's `url`, else the release URL).
    pub download_url: String,
    /// Where to fetch the product's own `.sha256` sidecar (always the release URL).
    pub sidecar_url: String,
    pub extra: Table,
}

impl Artifact {
    /// The asset's file name.
    pub fn name(&self) -> &str {
        &self.asset
    }
}

/// `[build.<tool>."<version>"]`: digests per target.
#[derive(Clone, Debug, PartialEq)]
pub struct BuildEntry {
    pub released: Option<String>,
    /// Keyed by target (`linux-x86_64`, `any`).
    pub artifacts: BTreeMap<String, Artifact>,
    pub extra: Table,
}

/// `[carton."X"]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Carton {
    pub version: CartonVersion,
    pub released: String,
    pub notes: String,
    pub tested: Vec<String>,
    /// tool name -> the exact build in this carton.
    pub products: BTreeMap<String, Version>,
    pub packs: BTreeMap<String, Vec<String>>,
    pub advisory: Option<String>,
}

/// A semver range as written (`>=1.16.1, <2`) and as parsed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Requirement {
    pub text: String,
    pub req: semver::VersionReq,
}

impl Requirement {
    pub fn parse(text: &str) -> Result<Requirement, String> {
        semver::VersionReq::parse(text)
            .map(|req| Requirement {
                text: text.trim().to_string(),
                req,
            })
            .map_err(|e| format!("`{text}` is not a version range: {e}"))
    }

    /// Prereleases never satisfy a range (semver's rule).
    pub fn matches(&self, v: &Version) -> bool {
        v.to_semver().is_some_and(|s| self.req.matches(&s))
    }

    /// Whether the version's core (major.minor.patch) is in range, a
    /// prerelease or not. For advisories: `<1.4.2` must flag `1.4.1-rc.2`.
    pub fn matches_core(&self, v: &Version) -> bool {
        v.to_semver().is_some_and(|mut s| {
            s.pre = semver::Prerelease::EMPTY;
            self.req.matches(&s)
        })
    }
}

/// A value under `embeds`: a number (`journal_format = 3`) or text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Embed {
    Int(i64),
    Text(String),
}

/// `[product.<tool>."<version>"]`: what a build needs and embeds.
#[derive(Clone, Debug, PartialEq)]
pub struct ProductReq {
    /// Hard requirements on other tools: refuse when unmet.
    pub requires: BTreeMap<String, Requirement>,
    /// Soft requirements (an extension of the contract): warn when unmet.
    pub recommends: BTreeMap<String, Requirement>,
    pub embeds: BTreeMap<String, Embed>,
    /// Free text per OS; evaluated only where it reads `kernel X.Y`.
    pub os_min: BTreeMap<String, String>,
    /// The build's license as an SPDX expression (`Apache-2.0`, `FSL-1.1-ALv2`). Optional: a
    /// manifest without it still works, and the installer says "not stated".
    pub license: Option<String>,
    pub extra: Table,
}

/// Whether `text` has the shape of an SPDX license expression: ids, `AND`, `OR`, `WITH`,
/// parentheses, `+`. It checks the shape only; it does not look ids up in the SPDX list.
pub fn is_spdx_expression(text: &str) -> bool {
    let t = text.trim();
    !t.is_empty()
        && t.len() <= 200
        && t.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+' | '(' | ')' | ' '))
        && t.chars().any(|c| c.is_ascii_alphanumeric())
}

impl ProductReq {
    /// `embeds.journal_format`, when it is a non-negative integer.
    pub fn journal_format(&self) -> Option<u32> {
        match self.embeds.get("journal_format") {
            Some(Embed::Int(n)) => u32::try_from(*n).ok(),
            _ => None,
        }
    }

    /// `embeds.cig_runtime` (the cig kernel a compiled program carries).
    pub fn cig_runtime(&self) -> Option<Version> {
        match self.embeds.get("cig_runtime") {
            Some(Embed::Text(t)) => Version::parse(t).ok(),
            _ => None,
        }
    }
}

/// What a yank or an advisory can point at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Yankable {
    Carton(CartonVersion),
    Product(String, Version),
}

impl fmt::Display for Yankable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Yankable::Carton(v) => write!(f, "carton {v}"),
            Yankable::Product(t, v) => write!(f, "{t} {v}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Yank {
    pub what: Yankable,
    pub reason: String,
    pub fixed_in: Option<Version>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdvisorySeverity {
    Security,
    Bug,
    Note,
}

impl AdvisorySeverity {
    pub fn as_str(self) -> &'static str {
        match self {
            AdvisorySeverity::Security => "security",
            AdvisorySeverity::Bug => "bug",
            AdvisorySeverity::Note => "note",
        }
    }
}

/// One `affects` entry: `carton <0.1.0`, `cigo >=1.0.0, <1.4.2`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Affects {
    /// `carton` or a tool name.
    pub subject: String,
    pub range: Requirement,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Advisory {
    pub id: String,
    pub affects: Vec<Affects>,
    pub fixed_in: Option<Version>,
    pub severity: AdvisorySeverity,
    pub summary: String,
    pub url: Option<String>,
}

/// What changed for one tool between two cartons.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolChange {
    pub tool: String,
    pub from: Option<Version>,
    pub to: Option<Version>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Removed,
    Upgraded,
    Downgraded,
}

impl ToolChange {
    pub fn kind(&self) -> ChangeKind {
        match (&self.from, &self.to) {
            (None, _) => ChangeKind::Added,
            (_, None) => ChangeKind::Removed,
            (Some(a), Some(b)) if b > a => ChangeKind::Upgraded,
            _ => ChangeKind::Downgraded,
        }
    }
}

/// The parsed, validated manifest for one channel.
#[derive(Clone, Debug, PartialEq)]
pub struct Manifest {
    pub format: u32,
    pub channel: String,
    pub sequence: u64,
    pub generated: String,
    pub expires: String,
    pub tools: BTreeMap<String, ToolDef>,
    pub cartons: BTreeMap<CartonVersion, Carton>,
    pub builds: BTreeMap<(String, Version), BuildEntry>,
    pub products: BTreeMap<(String, Version), ProductReq>,
    pub yanked: Vec<Yank>,
    pub advisories: Vec<Advisory>,
    /// `[archive]`: where the signed archive index is published, when the manifest names one.
    pub archive: Option<ArchiveRef>,
    /// Unknown top-level keys, kept (never fatal inside the same `format`).
    pub extra: Table,
}

// ---------------------------------------------------------------- parsing helpers

/// Shim names a manifest may never claim: `bin/ciginstaller` and `bin/cigi` are the installer's.
pub const RESERVED_SHIM_NAMES: &[&str] = &["ciginstaller", "cigi"];

fn name_ok(s: &str) -> bool {
    let mut c = s.chars();
    matches!(c.next(), Some('a'..='z'))
        && c.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// A relative path of plain components: non-empty, `/`-separated, no `.`/`..`/empty part, no
/// backslash, colon or control character. Used for every file path a manifest names.
pub fn is_plain_rel_path(s: &str) -> bool {
    !s.is_empty()
        && s.split('/').all(|c| {
            !c.is_empty()
                && c != "."
                && c != ".."
                && !c
                    .chars()
                    .any(|ch| ch == '\\' || ch == ':' || ch.is_control())
        })
}

fn as_table<'a>(v: &'a Value, path: &str) -> Result<&'a Table, ManifestError> {
    match v {
        Value::Table(t) => Ok(t),
        _ => err(format!("{path} must be a table")),
    }
}

fn req_str(t: &Table, key: &str, path: &str) -> Result<String, ManifestError> {
    match t.get(key) {
        Some(Value::String(s)) if !s.is_empty() => Ok(s.clone()),
        Some(_) => err(format!("{path}.{key} must be a non-empty string")),
        None => err(format!("{path}.{key} is missing")),
    }
}

fn opt_str(t: &Table, key: &str, path: &str) -> Result<Option<String>, ManifestError> {
    match t.get(key) {
        None => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => err(format!("{path}.{key} must be a string")),
    }
}

/// A date or date-time, as canonical text.
fn date_text(v: &Value, path: &str) -> Result<String, ManifestError> {
    match v {
        Value::Datetime(d) => Ok(d.to_string()),
        Value::String(s) => match s.parse::<toml::value::Datetime>() {
            Ok(d) => Ok(d.to_string()),
            Err(_) => err(format!("{path} is not a date")),
        },
        _ => err(format!("{path} must be a date")),
    }
}

fn date_value(s: &str) -> Value {
    match s.parse::<toml::value::Datetime>() {
        Ok(d) => Value::Datetime(d),
        Err(_) => Value::String(s.to_string()),
    }
}

fn parse_version(s: &str, path: &str) -> Result<Version, ManifestError> {
    Version::parse(s).or_else(|e| err(format!("{path}: {e}")))
}

fn expand(template: &str, vars: &[(&str, &str)]) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = template;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let Some(j) = after.find('}') else {
            return Err(format!("unterminated `{{` in `{template}`"));
        };
        let name = &after[..j];
        match vars.iter().find(|(k, _)| *k == name) {
            Some((_, v)) => out.push_str(v),
            None => return Err(format!("`{{{name}}}` cannot be filled in `{template}`")),
        }
        rest = &after[j + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

fn parse_tool(name: &str, v: &Value) -> Result<ToolDef, ManifestError> {
    let path = format!("tool.{name}");
    if !name_ok(name) {
        return err(format!("{path}: tool names are [a-z][a-z0-9-]*"));
    }
    let t = as_table(v, &path)?;
    let layout = match req_str(t, "layout", &path)?.as_str() {
        "binary" => Layout::Binary,
        "archive" => Layout::Archive,
        other => return err(format!("{path}.layout `{other}` is not binary or archive")),
    };
    let asset = req_str(t, "asset", &path)?;
    let sidecar = opt_str(t, "sidecar", &path)?.unwrap_or_else(|| "{asset}.sha256".to_string());
    let strip = match t.get("strip") {
        None => 0,
        Some(Value::Integer(n)) => match u32::try_from(*n) {
            Ok(n) => n,
            Err(_) => return err(format!("{path}.strip is out of range")),
        },
        Some(_) => return err(format!("{path}.strip must be an integer")),
    };
    let mut shims = Vec::new();
    match t.get("shims") {
        None => {}
        Some(Value::Array(a)) => {
            for (i, s) in a.iter().enumerate() {
                let sp = format!("{path}.shims[{i}]");
                let st = as_table(s, &sp)?;
                let def = ShimDef {
                    name: req_str(st, "name", &sp)?,
                    path: req_str(st, "path", &sp)?,
                    windows_path: opt_str(st, "windows_path", &sp)?,
                };
                // the name becomes a file in the home's bin/, first on PATH: a plain command
                // name, never a path, and never the installer's own
                if !name_ok(&def.name) || RESERVED_SHIM_NAMES.contains(&def.name.as_str()) {
                    return err(format!(
                        "{sp}.name {:?} must be a command name of [a-z][a-z0-9-]*, and not {}",
                        def.name,
                        RESERVED_SHIM_NAMES.join(" or ")
                    ));
                }
                for (key, val) in [
                    ("path", Some(&def.path)),
                    ("windows_path", def.windows_path.as_ref()),
                ] {
                    if let Some(v) = val {
                        if !is_plain_rel_path(v) {
                            return err(format!(
                                "{sp}.{key} {v:?} must be a relative path of plain components (no `..`, no absolute path)"
                            ));
                        }
                    }
                }
                shims.push(def);
            }
        }
        Some(_) => return err(format!("{path}.shims must be an array")),
    }
    let targets = match t.get("targets") {
        Some(Value::Array(a)) if !a.is_empty() => {
            let mut out = Vec::new();
            for x in a {
                match x {
                    Value::String(s) if s == "any" || Target::parse(s).is_some() => {
                        out.push(s.clone())
                    }
                    _ => return err(format!("{path}.targets holds a bad target")),
                }
            }
            out
        }
        _ => return err(format!("{path}.targets must be a non-empty array")),
    };
    let attests = match t.get("attests") {
        None => false,
        Some(Value::Boolean(b)) => *b,
        Some(_) => return err(format!("{path}.attests must be a boolean")),
    };
    if layout == Layout::Binary && shims.len() != 1 {
        return err(format!("{path}: a binary layout needs exactly one shim"));
    }
    let known = [
        "repo", "tag", "layout", "asset", "sidecar", "strip", "shims", "targets", "attests",
    ];
    let extra = t
        .iter()
        .filter(|(k, _)| !known.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    Ok(ToolDef {
        repo: req_str(t, "repo", &path)?,
        tag: req_str(t, "tag", &path)?,
        layout,
        asset,
        sidecar,
        strip,
        shims,
        targets,
        attests,
        extra,
    })
}

fn parse_artifact(
    tool: &str,
    def: &ToolDef,
    version: &Version,
    target: &str,
    t: &Table,
    path: &str,
) -> Result<Artifact, ManifestError> {
    let sha256 = req_str(t, "sha256", path)?;
    if sha256.len() != 64
        || !sha256
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    {
        return err(format!("{path}.sha256 must be 64 lowercase hex digits"));
    }
    let size = match t.get("size") {
        Some(Value::Integer(n)) if (0..).contains(n) => *n as u64,
        _ => return err(format!("{path}.size must be a non-negative integer")),
    };
    if size >= MAX_SIZE {
        return err(format!("{path}.size is 1 GB or more"));
    }
    let url = opt_str(t, "url", path)?;
    let ReleaseNames {
        tag,
        asset,
        sidecar,
    } = def
        .names(version, target)
        .or_else(|e| err(format!("{path}: {e}")))?;
    let base = format!("https://github.com/{}/releases/download/{tag}", def.repo);
    let known = ["sha256", "size", "url"];
    let extra = t
        .iter()
        .filter(|(k, _)| !known.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    Ok(Artifact {
        tool: tool.to_string(),
        version: version.clone(),
        target: target.to_string(),
        sha256,
        size,
        download_url: url.clone().unwrap_or_else(|| format!("{base}/{asset}")),
        sidecar_url: format!("{base}/{sidecar}"),
        url,
        repo: def.repo.clone(),
        tag,
        layout: def.layout,
        strip: def.strip,
        attests: def.attests,
        asset,
        sidecar,
        extra,
    })
}

fn parse_requirements(
    t: &Table,
    key: &str,
    path: &str,
    tools: &BTreeMap<String, ToolDef>,
) -> Result<BTreeMap<String, Requirement>, ManifestError> {
    let mut out = BTreeMap::new();
    let Some(v) = t.get(key) else {
        return Ok(out);
    };
    let rt = as_table(v, &format!("{path}.{key}"))?;
    for (dep, r) in rt {
        let Value::String(text) = r else {
            return err(format!("{path}.{key}.{dep} must be a version range string"));
        };
        if !tools.contains_key(dep) {
            return err(format!("{path}.{key}.{dep}: no such tool is defined"));
        }
        match Requirement::parse(text) {
            Ok(r) => out.insert(dep.clone(), r),
            Err(e) => return err(format!("{path}.{key}.{dep}: {e}")),
        };
    }
    Ok(out)
}

fn parse_subject_version(
    key: &str,
    tools: &BTreeMap<String, ToolDef>,
    path: &str,
) -> Result<Yankable, ManifestError> {
    let Some((subject, ver)) = key.split_once(' ') else {
        return err(format!(
            "{path}: `{key}` must read `<tool|carton> <version>`"
        ));
    };
    let v = parse_version(ver.trim(), path)?;
    if subject == "carton" {
        Ok(Yankable::Carton(v))
    } else if tools.contains_key(subject) {
        Ok(Yankable::Product(subject.to_string(), v))
    } else {
        err(format!("{path}: no such tool `{subject}`"))
    }
}

fn duplicate_check<K: Ord + fmt::Debug>(
    map: &mut BTreeMap<K, ()>,
    key: K,
    what: &str,
) -> Result<(), ManifestError> {
    if map.insert(key, ()).is_some() {
        return err(format!("duplicate {what}"));
    }
    Ok(())
}

impl Manifest {
    /// Parse and validate (section 6.2). Every refusal is `E1825`.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(text: &str) -> Result<Manifest, ManifestError> {
        if text.len() > MAX_TEXT {
            return err("the manifest is larger than 16 MiB");
        }
        let root: Table = match text.parse() {
            Ok(t) => t,
            Err(e) => return err(format!("not valid TOML: {e}")),
        };
        let format = match root.get("format") {
            Some(Value::Integer(1)) => 1u32,
            Some(Value::Integer(n)) if *n > 1 => {
                return err(format!("format {n} is newer than this ciginstaller reads"))
            }
            _ => return err("`format` must be 1"),
        };
        let channel = req_str(&root, "channel", "manifest")?;
        let sequence = match root.get("sequence") {
            Some(Value::Integer(n)) if *n >= 0 => *n as u64,
            _ => return err("`sequence` must be a non-negative integer"),
        };
        let generated = date_text(
            root.get("generated")
                .ok_or(())
                .or_else(|_| err("`generated` is missing"))?,
            "generated",
        )?;
        let expires = date_text(
            root.get("expires")
                .ok_or(())
                .or_else(|_| err("`expires` is missing"))?,
            "expires",
        )?;
        match (rfc3339_to_epoch(&generated), rfc3339_to_epoch(&expires)) {
            (Some(g), Some(e)) if e > g => {}
            (Some(_), Some(_)) => return err("`expires` must be after `generated`"),
            _ => return err("`generated` and `expires` must be full date-times with an offset"),
        }

        let mut tools = BTreeMap::new();
        let mut shim_owner: BTreeMap<String, String> = BTreeMap::new();
        if let Some(v) = root.get("tool") {
            for (name, def) in as_table(v, "tool")? {
                let def = parse_tool(name, def)?;
                for s in &def.shims {
                    if let Some(other) = shim_owner.insert(s.name.clone(), name.clone()) {
                        return err(format!(
                            "tool.{name}: the shim `{}` is already tool.{other}'s",
                            s.name
                        ));
                    }
                }
                tools.insert(name.clone(), def);
            }
        }

        let mut builds = BTreeMap::new();
        let mut seen = BTreeMap::new();
        if let Some(v) = root.get("build") {
            for (tool, versions) in as_table(v, "build")? {
                let Some(def) = tools.get(tool) else {
                    return err(format!("build.{tool}: no such tool is defined"));
                };
                for (vtext, entry) in as_table(versions, &format!("build.{tool}"))? {
                    let path = format!("build.{tool}.\"{vtext}\"");
                    let version = parse_version(vtext, &path)?;
                    duplicate_check(&mut seen, (tool.clone(), version.clone()), &path)?;
                    let et = as_table(entry, &path)?;
                    let mut released = None;
                    let mut artifacts = BTreeMap::new();
                    let mut extra = Table::new();
                    for (k, val) in et {
                        if k == "released" {
                            released = Some(date_text(val, &format!("{path}.released"))?);
                        } else if let Value::Table(tt) = val {
                            let ok = if k == "any" {
                                def.targets.iter().any(|t| t == "any")
                            } else {
                                def.targets.iter().any(|t| t == k)
                            };
                            if !ok {
                                return err(format!("{path}.{k}: not a target {tool} declares"));
                            }
                            let ap = format!("{path}.{k}");
                            artifacts.insert(
                                k.clone(),
                                parse_artifact(tool, def, &version, k, tt, &ap)?,
                            );
                        } else {
                            extra.insert(k.clone(), val.clone());
                        }
                    }
                    builds.insert(
                        (tool.clone(), version),
                        BuildEntry {
                            released,
                            artifacts,
                            extra,
                        },
                    );
                }
            }
        }

        let mut cartons = BTreeMap::new();
        if let Some(v) = root.get("carton") {
            for (vtext, ct) in as_table(v, "carton")? {
                let path = format!("carton.\"{vtext}\"");
                let version = parse_version(vtext, &path)?;
                if cartons.contains_key(&version) {
                    return err(format!("duplicate carton {version}"));
                }
                let t = as_table(ct, &path)?;
                let mut carton = Carton {
                    version: version.clone(),
                    released: match t.get("released") {
                        Some(d) => date_text(d, &format!("{path}.released"))?,
                        None => String::new(),
                    },
                    notes: opt_str(t, "notes", &path)?.unwrap_or_default(),
                    tested: Vec::new(),
                    products: BTreeMap::new(),
                    packs: BTreeMap::new(),
                    advisory: opt_str(t, "advisory", &path)?,
                };
                match t.get("tested") {
                    None => {}
                    Some(Value::Array(a)) => {
                        for x in a {
                            match x {
                                Value::String(s) => carton.tested.push(s.clone()),
                                _ => return err(format!("{path}.tested holds a non-string")),
                            }
                        }
                    }
                    Some(_) => return err(format!("{path}.tested must be an array")),
                }
                for (k, val) in t {
                    if RESERVED_CARTON_KEYS.contains(&k.as_str()) {
                        continue;
                    }
                    if !tools.contains_key(k) {
                        return err(format!("{path}.{k}: not a reserved key or a defined tool"));
                    }
                    let Value::String(s) = val else {
                        return err(format!("{path}.{k} must be an exact version string"));
                    };
                    let pv = parse_version(s, &format!("{path}.{k}"))?;
                    if !builds.contains_key(&(k.clone(), pv.clone())) {
                        return err(format!("{path}.{k}: no [build.{k}.\"{pv}\"] table"));
                    }
                    carton.products.insert(k.clone(), pv);
                }
                if let Some(p) = t.get("packs") {
                    for (name, members) in as_table(p, &format!("{path}.packs"))? {
                        if !name_ok(name) {
                            return err(format!("{path}.packs.{name}: bad pack name"));
                        }
                        let Value::Array(a) = members else {
                            return err(format!("{path}.packs.{name} must be an array"));
                        };
                        let mut list = Vec::new();
                        for m in a {
                            match m {
                                Value::String(s) if carton.products.contains_key(s) => {
                                    list.push(s.clone())
                                }
                                _ => {
                                    return err(format!(
                                    "{path}.packs.{name}: members must be products of this carton"
                                ))
                                }
                            }
                        }
                        carton.packs.insert(name.clone(), list);
                    }
                }
                cartons.insert(version, carton);
            }
        }

        let mut products = BTreeMap::new();
        if let Some(v) = root.get("product") {
            for (tool, versions) in as_table(v, "product")? {
                if !tools.contains_key(tool) {
                    return err(format!("product.{tool}: no such tool is defined"));
                }
                for (vtext, pt) in as_table(versions, &format!("product.{tool}"))? {
                    let path = format!("product.{tool}.\"{vtext}\"");
                    let version = parse_version(vtext, &path)?;
                    if products.contains_key(&(tool.clone(), version.clone())) {
                        return err(format!("duplicate {path}"));
                    }
                    let t = as_table(pt, &path)?;
                    let mut embeds = BTreeMap::new();
                    if let Some(e) = t.get("embeds") {
                        for (k, val) in as_table(e, &format!("{path}.embeds"))? {
                            embeds.insert(
                                k.clone(),
                                match val {
                                    Value::Integer(n) => Embed::Int(*n),
                                    Value::String(s) => Embed::Text(s.clone()),
                                    _ => {
                                        return err(format!(
                                            "{path}.embeds.{k} must be a number or a string"
                                        ))
                                    }
                                },
                            );
                        }
                    }
                    let mut os_min = BTreeMap::new();
                    if let Some(e) = t.get("os_min") {
                        for (k, val) in as_table(e, &format!("{path}.os_min"))? {
                            match val {
                                Value::String(s) => os_min.insert(k.clone(), s.clone()),
                                _ => return err(format!("{path}.os_min.{k} must be a string")),
                            };
                        }
                    }
                    let license = opt_str(t, "license", &path)?;
                    if let Some(l) = &license {
                        if !is_spdx_expression(l) {
                            return err(format!(
                                "{path}.license `{l}` is not an SPDX license expression (such as `Apache-2.0` or `FSL-1.1-ALv2`)"
                            ));
                        }
                    }
                    let known = ["requires", "recommends", "embeds", "os_min", "license"];
                    products.insert(
                        (tool.clone(), version),
                        ProductReq {
                            requires: parse_requirements(t, "requires", &path, &tools)?,
                            recommends: parse_requirements(t, "recommends", &path, &tools)?,
                            embeds,
                            os_min,
                            license,
                            extra: t
                                .iter()
                                .filter(|(k, _)| !known.contains(&k.as_str()))
                                .map(|(k, v)| (k.clone(), v.clone()))
                                .collect(),
                        },
                    );
                }
            }
        }

        let mut yanked = Vec::new();
        if let Some(v) = root.get("yanked") {
            for (key, val) in as_table(v, "yanked")? {
                let path = format!("yanked.\"{key}\"");
                let what = parse_subject_version(key, &tools, &path)?;
                let (reason, fixed_in) = match val {
                    Value::String(s) if !s.is_empty() => (s.clone(), None),
                    Value::String(_) => return err(format!("{path} needs a reason")),
                    Value::Table(t) => (
                        req_str(t, "reason", &path)?,
                        match opt_str(t, "fixed_in", &path)? {
                            Some(s) => Some(parse_version(&s, &path)?),
                            None => None,
                        },
                    ),
                    _ => return err(format!("{path} must be a reason or a table")),
                };
                yanked.push(Yank {
                    what,
                    reason,
                    fixed_in,
                });
            }
            yanked.sort_by_key(|y| y.what.to_string());
            for w in yanked.windows(2) {
                if w[0].what == w[1].what {
                    return err(format!("duplicate yank of {}", w[0].what));
                }
            }
        }

        let mut advisories = Vec::new();
        match root.get("advisory") {
            None => {}
            Some(Value::Array(a)) => {
                for (i, item) in a.iter().enumerate() {
                    let path = format!("advisory[{i}]");
                    let t = as_table(item, &path)?;
                    let mut affects = Vec::new();
                    match t.get("affects") {
                        Some(Value::Array(list)) => {
                            for x in list {
                                let Value::String(s) = x else {
                                    return err(format!("{path}.affects holds a non-string"));
                                };
                                let Some((subject, range)) = s.split_once(' ') else {
                                    return err(format!(
                                        "{path}.affects `{s}` must read `<tool|carton> <range>`"
                                    ));
                                };
                                if subject != "carton" && !tools.contains_key(subject) {
                                    return err(format!(
                                        "{path}.affects: no such tool `{subject}`"
                                    ));
                                }
                                match Requirement::parse(range) {
                                    Ok(range) => affects.push(Affects {
                                        subject: subject.to_string(),
                                        range,
                                    }),
                                    Err(e) => return err(format!("{path}.affects: {e}")),
                                }
                            }
                        }
                        _ => return err(format!("{path}.affects must be an array")),
                    }
                    let severity = match req_str(t, "severity", &path)?.as_str() {
                        "security" => AdvisorySeverity::Security,
                        "bug" => AdvisorySeverity::Bug,
                        "note" => AdvisorySeverity::Note,
                        other => {
                            return err(format!(
                                "{path}.severity `{other}` is not security, bug or note"
                            ))
                        }
                    };
                    advisories.push(Advisory {
                        id: req_str(t, "id", &path)?,
                        affects,
                        fixed_in: match opt_str(t, "fixed_in", &path)? {
                            Some(s) => Some(parse_version(&s, &path)?),
                            None => None,
                        },
                        severity,
                        summary: opt_str(t, "summary", &path)?.unwrap_or_default(),
                        url: opt_str(t, "url", &path)?,
                    });
                }
            }
            Some(_) => return err("`advisory` must be an array of tables"),
        }

        let known = [
            "format",
            "channel",
            "sequence",
            "generated",
            "expires",
            "tool",
            "build",
            "carton",
            "product",
            "yanked",
            "advisory",
            "archive",
        ];
        let archive = match root.get("archive") {
            None => None,
            Some(v) => {
                let t = as_table(v, "archive")?;
                let index = req_str(t, "index", "archive")?;
                Some(ArchiveRef::parse(&index).or_else(|e| err(format!("archive.index: {e}")))?)
            }
        };
        let extra = root
            .iter()
            .filter(|(k, _)| !known.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        Ok(Manifest {
            format,
            channel,
            sequence,
            generated,
            expires,
            tools,
            cartons,
            builds,
            products,
            yanked,
            advisories,
            archive,
            extra,
        })
    }

    /// Notes for unknown keys that were kept (shown as warnings, never fatal).
    pub fn warnings(&self) -> Vec<String> {
        let mut out = Vec::new();
        for k in self.extra.keys() {
            out.push(format!("unknown top-level key `{k}` kept"));
        }
        for (n, t) in &self.tools {
            for k in t.extra.keys() {
                out.push(format!("unknown key `tool.{n}.{k}` kept"));
            }
        }
        for ((n, v), b) in &self.builds {
            for k in b.extra.keys() {
                out.push(format!("unknown key `build.{n}.\"{v}\".{k}` kept"));
            }
        }
        for ((n, v), p) in &self.products {
            for k in p.extra.keys() {
                out.push(format!("unknown key `product.{n}.\"{v}\".{k}` kept"));
            }
        }
        out
    }

    /// Seconds since the epoch at which this manifest expires.
    pub fn expires_epoch(&self) -> Option<i64> {
        rfc3339_to_epoch(&self.expires)
    }

    /// True when `now_epoch` (seconds) is at or past `expires`.
    pub fn is_expired(&self, now_epoch: i64) -> bool {
        self.expires_epoch().is_none_or(|e| now_epoch >= e)
    }

    /// The manifest as TOML text; `from_str(render(m)) == m`.
    pub fn render(&self) -> String {
        let mut root = self.extra.clone();
        root.insert("format".into(), Value::Integer(i64::from(self.format)));
        root.insert("channel".into(), Value::String(self.channel.clone()));
        root.insert(
            "sequence".into(),
            Value::Integer(i64::try_from(self.sequence).unwrap_or(i64::MAX)),
        );
        root.insert("generated".into(), date_value(&self.generated));
        root.insert("expires".into(), date_value(&self.expires));

        let mut tools = Table::new();
        for (name, d) in &self.tools {
            let mut t = d.extra.clone();
            t.insert("repo".into(), Value::String(d.repo.clone()));
            t.insert("tag".into(), Value::String(d.tag.clone()));
            t.insert("layout".into(), Value::String(d.layout.as_str().into()));
            t.insert("asset".into(), Value::String(d.asset.clone()));
            t.insert("sidecar".into(), Value::String(d.sidecar.clone()));
            t.insert("strip".into(), Value::Integer(i64::from(d.strip)));
            let shims = d
                .shims
                .iter()
                .map(|s| {
                    let mut st = Table::new();
                    st.insert("name".into(), Value::String(s.name.clone()));
                    st.insert("path".into(), Value::String(s.path.clone()));
                    if let Some(w) = &s.windows_path {
                        st.insert("windows_path".into(), Value::String(w.clone()));
                    }
                    Value::Table(st)
                })
                .collect();
            t.insert("shims".into(), Value::Array(shims));
            t.insert(
                "targets".into(),
                Value::Array(d.targets.iter().cloned().map(Value::String).collect()),
            );
            t.insert("attests".into(), Value::Boolean(d.attests));
            tools.insert(name.clone(), Value::Table(t));
        }
        if !tools.is_empty() {
            root.insert("tool".into(), Value::Table(tools));
        }

        let mut builds = Table::new();
        for ((tool, v), b) in &self.builds {
            let mut bt = b.extra.clone();
            if let Some(r) = &b.released {
                bt.insert("released".into(), date_value(r));
            }
            for (target, a) in &b.artifacts {
                let mut at = a.extra.clone();
                at.insert("sha256".into(), Value::String(a.sha256.clone()));
                at.insert("size".into(), Value::Integer(a.size as i64));
                if let Some(u) = &a.url {
                    at.insert("url".into(), Value::String(u.clone()));
                }
                bt.insert(target.clone(), Value::Table(at));
            }
            nested(&mut builds, tool, &v.to_string(), bt);
        }
        if !builds.is_empty() {
            root.insert("build".into(), Value::Table(builds));
        }

        let mut cartons = Table::new();
        for (v, c) in &self.cartons {
            let mut t = Table::new();
            if !c.released.is_empty() {
                t.insert("released".into(), date_value(&c.released));
            }
            if !c.notes.is_empty() {
                t.insert("notes".into(), Value::String(c.notes.clone()));
            }
            if !c.tested.is_empty() {
                t.insert(
                    "tested".into(),
                    Value::Array(c.tested.iter().cloned().map(Value::String).collect()),
                );
            }
            if let Some(a) = &c.advisory {
                t.insert("advisory".into(), Value::String(a.clone()));
            }
            for (tool, pv) in &c.products {
                t.insert(tool.clone(), Value::String(pv.to_string()));
            }
            if !c.packs.is_empty() {
                let mut packs = Table::new();
                for (name, members) in &c.packs {
                    packs.insert(
                        name.clone(),
                        Value::Array(members.iter().cloned().map(Value::String).collect()),
                    );
                }
                t.insert("packs".into(), Value::Table(packs));
            }
            cartons.insert(v.to_string(), Value::Table(t));
        }
        if !cartons.is_empty() {
            root.insert("carton".into(), Value::Table(cartons));
        }

        let mut products = Table::new();
        for ((tool, v), p) in &self.products {
            let mut t = p.extra.clone();
            for (key, map) in [("requires", &p.requires), ("recommends", &p.recommends)] {
                if !map.is_empty() {
                    let mut rt = Table::new();
                    for (dep, r) in map {
                        rt.insert(dep.clone(), Value::String(r.text.clone()));
                    }
                    t.insert(key.into(), Value::Table(rt));
                }
            }
            if !p.embeds.is_empty() {
                let mut et = Table::new();
                for (k, e) in &p.embeds {
                    et.insert(
                        k.clone(),
                        match e {
                            Embed::Int(n) => Value::Integer(*n),
                            Embed::Text(s) => Value::String(s.clone()),
                        },
                    );
                }
                t.insert("embeds".into(), Value::Table(et));
            }
            if !p.os_min.is_empty() {
                let mut ot = Table::new();
                for (k, s) in &p.os_min {
                    ot.insert(k.clone(), Value::String(s.clone()));
                }
                t.insert("os_min".into(), Value::Table(ot));
            }
            if let Some(l) = &p.license {
                t.insert("license".into(), Value::String(l.clone()));
            }
            nested(&mut products, tool, &v.to_string(), t);
        }
        if !products.is_empty() {
            root.insert("product".into(), Value::Table(products));
        }

        if !self.yanked.is_empty() {
            let mut yt = Table::new();
            for y in &self.yanked {
                let val = match &y.fixed_in {
                    None => Value::String(y.reason.clone()),
                    Some(f) => {
                        let mut t = Table::new();
                        t.insert("reason".into(), Value::String(y.reason.clone()));
                        t.insert("fixed_in".into(), Value::String(f.to_string()));
                        Value::Table(t)
                    }
                };
                yt.insert(y.what.to_string(), val);
            }
            root.insert("yanked".into(), Value::Table(yt));
        }

        if !self.advisories.is_empty() {
            let list = self
                .advisories
                .iter()
                .map(|a| {
                    let mut t = Table::new();
                    t.insert("id".into(), Value::String(a.id.clone()));
                    t.insert(
                        "affects".into(),
                        Value::Array(
                            a.affects
                                .iter()
                                .map(|x| Value::String(format!("{} {}", x.subject, x.range.text)))
                                .collect(),
                        ),
                    );
                    if let Some(f) = &a.fixed_in {
                        t.insert("fixed_in".into(), Value::String(f.to_string()));
                    }
                    t.insert("severity".into(), Value::String(a.severity.as_str().into()));
                    t.insert("summary".into(), Value::String(a.summary.clone()));
                    if let Some(u) = &a.url {
                        t.insert("url".into(), Value::String(u.clone()));
                    }
                    Value::Table(t)
                })
                .collect();
            root.insert("advisory".into(), Value::Array(list));
        }
        if let Some(a) = &self.archive {
            let mut t = Table::new();
            t.insert("index".into(), Value::String(a.index.clone()));
            root.insert("archive".into(), Value::Table(t));
        }
        toml::to_string(&root).unwrap_or_default()
    }

    // ------------------------------------------------------------------ queries

    pub fn carton(&self, v: &CartonVersion) -> Option<&Carton> {
        self.cartons.get(v)
    }

    pub fn tool(&self, name: &str) -> Option<&ToolDef> {
        self.tools.get(name)
    }

    pub fn product(&self, tool: &str, v: &Version) -> Option<&ProductReq> {
        self.products.get(&(tool.to_string(), v.clone()))
    }

    pub fn is_carton_yanked(&self, v: &CartonVersion) -> bool {
        self.yank_of(Yankable::Carton(v.clone())).is_some()
    }

    pub fn is_product_yanked(&self, tool: &str, v: &Version) -> bool {
        self.yank_of(Yankable::Product(tool.to_string(), v.clone()))
            .is_some()
    }

    /// The highest carton that is not yanked; prereleases only with `include_pre`.
    pub fn latest(&self, include_pre: bool) -> Option<&Carton> {
        self.cartons.values().rev().find(|c| {
            (include_pre || !c.version.is_prerelease()) && !self.is_carton_yanked(&c.version)
        })
    }

    /// The newest carton the pin accepts (restricted to `installed_only` when given).
    /// A carton that is not yanked wins over a yanked one; only an exact pin may
    /// name a yanked carton (`check` warns about it). A `1.1` or `1` pin whose
    /// only matches are yanked finds none.
    pub fn newest_matching(
        &self,
        pin: &Pin,
        installed_only: Option<&[CartonVersion]>,
    ) -> Option<&Carton> {
        let fits = |c: &&Carton| {
            pin.satisfied_by(&c.version)
                && installed_only.is_none_or(|list| list.contains(&c.version))
        };
        let mut all = self.cartons.values().rev().filter(fits);
        let first = all.clone().find(|c| !self.is_carton_yanked(&c.version));
        // only an exact pin may name a yanked carton (`check` warns about it)
        first.or_else(|| {
            if matches!(pin, Pin::Exact(_)) {
                all.next()
            } else {
                None
            }
        })
    }

    /// "Which cigo is in carton 1.1.2?"
    pub fn build_in(&self, carton: &CartonVersion, tool: &str) -> Option<&Version> {
        self.cartons.get(carton)?.products.get(tool)
    }

    /// The download for a build on a target: the exact target, else `any`.
    pub fn artifact(&self, tool: &str, v: &Version, target: &Target) -> Option<&Artifact> {
        let b = self.builds.get(&(tool.to_string(), v.clone()))?;
        b.artifacts
            .get(&target.to_string())
            .or_else(|| b.artifacts.get("any"))
    }

    pub fn pack(&self, carton: &CartonVersion, name: &str) -> Option<&[String]> {
        self.cartons.get(carton)?.packs.get(name).map(Vec::as_slice)
    }

    /// Only the tools whose build differs between two cartons (empty when either is unknown).
    pub fn diff(&self, from: &CartonVersion, to: &CartonVersion) -> Vec<ToolChange> {
        let (Some(a), Some(b)) = (self.cartons.get(from), self.cartons.get(to)) else {
            return Vec::new();
        };
        let mut names: Vec<&String> = a.products.keys().chain(b.products.keys()).collect();
        names.sort();
        names.dedup();
        names
            .into_iter()
            .filter_map(|n| {
                let (x, y) = (a.products.get(n), b.products.get(n));
                (x != y).then(|| ToolChange {
                    tool: n.clone(),
                    from: x.cloned(),
                    to: y.cloned(),
                })
            })
            .collect()
    }

    pub fn yank_of(&self, what: Yankable) -> Option<&Yank> {
        self.yanked.iter().find(|y| y.what == what)
    }

    /// Every build of a tool, newest first.
    pub fn builds_of(&self, tool: &str) -> Vec<&Version> {
        let mut out: Vec<&Version> = self
            .builds
            .keys()
            .filter(|(t, _)| t == tool)
            .map(|(_, v)| v)
            .collect();
        out.sort_by(|a, b| b.cmp(a));
        out
    }
}

/// Insert `table` at `outer.<tool>."<version>"`.
fn nested(outer: &mut Table, tool: &str, version: &str, table: Table) {
    let slot = outer
        .entry(tool.to_string())
        .or_insert_with(|| Value::Table(Table::new()));
    if let Value::Table(t) = slot {
        t.insert(version.to_string(), Value::Table(table));
    }
}
