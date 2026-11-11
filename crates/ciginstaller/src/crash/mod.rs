// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! The family ashtray, read and written the way cig does (#10, #28 part 1): the same folder
//! (`<home>/ashtray`, and the older `<home>/crashes`), the same file per report, the same fields,
//! so a crash from any product lands in one queue and `ciginstaller crash list` shows them all.
//!
//! The installer invents no second format. [`Report`] is cig's `CrashReport`, field for field;
//! a field this build does not know is kept as it was read and written back unchanged, so a
//! report cig wrote survives an installer that touched it.

pub mod anon;
pub mod blame;
pub mod block;
pub mod health;
pub mod hook;
pub mod route;
pub mod routes;

use std::fs;
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};

use cigi_home::Config;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The tool that made the installer's own reports.
pub const ORIGIN: &str = "ciginstaller";

/// What kind of report. The same words as cig's (an unknown one is read as a crash).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Crash,
    Internal,
    Tripwire,
    Differential,
    Update,
    Report,
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Kind::Crash => "crash",
            Kind::Internal => "internal",
            Kind::Tripwire => "tripwire",
            Kind::Differential => "differential",
            Kind::Update => "update",
            Kind::Report => "report",
        }
    }
    pub fn parse(s: &str) -> Kind {
        match s {
            "internal" => Kind::Internal,
            "tripwire" => Kind::Tripwire,
            "differential" => Kind::Differential,
            "update" => Kind::Update,
            "report" => Kind::Report,
            _ => Kind::Crash,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Issue {
    pub repo: String,
    pub number: u64,
    pub url: String,
}

/// The ashtray's own bookkeeping; it stays home, never travels.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Queue {
    #[serde(default)]
    pub requested: bool,
    #[serde(default)]
    pub attempts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_try: Option<String>,
}

fn default_origin() -> String {
    "cig".to_string()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Report {
    pub id: String,
    pub fingerprint: String,
    pub version: String,
    pub os: String,
    pub arch: String,
    pub timestamp: String,
    pub command: Vec<String>,
    pub cwd: String,
    pub panic_message: String,
    pub panic_location: String,
    pub backtrace: Vec<String>,
    #[serde(default)]
    pub active_run_id: Option<String>,
    #[serde(default)]
    pub sent: Option<String>,
    #[serde(default)]
    pub rolled_back: Option<bool>,
    #[serde(default)]
    pub kind: Kind,
    #[serde(default = "default_origin")]
    pub origin: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repro: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<Issue>,
    #[serde(default)]
    pub queue: Queue,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed_in: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub announced: bool,
    /// Fields a newer cig or another product wrote that this build does not know.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The repository a product's reports go to when the report names none.
pub fn repo_of_origin(origin: &str) -> &'static str {
    match origin {
        "ciginstaller" => "otmof-ops/cig-installer",
        "cig-orchestrator" | "cigo" => "otmof-ops/cig-orchestrator",
        "cigcompile" | "cig-compile" => "otmof-ops/cig-compile",
        "cigar-script" | "cigar" | "cigarscript" => "otmof-ops/cigar-script",
        _ => "otmof-ops/cig-script",
    }
}

impl Report {
    /// The one line that says what happened.
    pub fn headline(&self) -> &str {
        self.title.as_deref().unwrap_or(&self.panic_message)
    }

    /// `filed`, `failed: <why>`, `pending` (asked for, waiting) or `local`.
    pub fn state(&self) -> String {
        if self.issue.is_some() || self.sent.is_some() {
            return "filed".into();
        }
        if let Some(e) = &self.queue.last_error {
            return format!("failed: {}", truncate(e, 60));
        }
        if self.queue.requested {
            return "pending".into();
        }
        "local".into()
    }

    pub fn filed(&self) -> bool {
        self.issue.is_some() || self.sent.is_some()
    }
}

pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{cut}…")
}

/// A report, anonymised and fingerprinted (#28 section 3), made by `origin` at `version`: the
/// installer's own, or another product's handed in through `crash submit`.
pub fn new_report(
    origin: &str,
    version: &str,
    kind: Kind,
    location: &str,
    message: &str,
    backtrace: Vec<String>,
    root: Option<&Path>,
) -> Report {
    let now = cigi_home::now_rfc3339();
    let message = truncate(&anon::anonymise_text_in(message, root), 500);
    let fingerprint = anon::fingerprint(origin, version, kind.word(), location, &message);
    let stamp: String = now.chars().filter(char::is_ascii_alphanumeric).collect();
    Report {
        id: format!("{stamp}-{}", &fingerprint[..6]),
        fingerprint,
        version: version.to_string(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        timestamp: now,
        command: anon::redact_command(std::env::args()),
        cwd: std::env::current_dir()
            .map(|p| anon::shape(&p, root))
            .unwrap_or_else(|_| "?".into()),
        panic_message: message,
        panic_location: location.to_string(),
        backtrace,
        active_run_id: None,
        sent: None,
        rolled_back: None,
        kind,
        origin: origin.to_string(),
        repo: None,
        title: None,
        details: None,
        repro: None,
        issue: None,
        queue: Queue::default(),
        fixed_in: None,
        announced: false,
        extra: Map::new(),
    }
}

// ----- the files --------------------------------------------------------------------------

pub fn ashtray_dir(root: &Path) -> PathBuf {
    root.join("ashtray")
}

/// Where reports were kept before cig 1.7.0; still read, still deleted from.
pub fn crashes_dir(root: &Path) -> PathBuf {
    root.join("crashes")
}

pub fn path_of(root: &Path, r: &Report) -> PathBuf {
    let name = format!("{}.json", r.id);
    let old = crashes_dir(root).join(&name);
    if old.is_file() {
        old
    } else {
        ashtray_dir(root).join(name)
    }
}

pub fn write(root: &Path, r: &Report) -> io::Result<PathBuf> {
    let path = path_of(root, r);
    let dir = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| ashtray_dir(root));
    fs::create_dir_all(&dir)?;
    let json = serde_json::to_string_pretty(r).map_err(io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json)?;
    fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Save a new report under a name of its own: the id comes from the second and the fingerprint,
/// so the same fault twice in one second would share it. The name is claimed by linking the
/// written file into place (which fails when taken) and the next suffix is tried.
pub fn write_new(root: &Path, r: &mut Report) -> io::Result<PathBuf> {
    let base = r.id.clone();
    let dir = ashtray_dir(root);
    fs::create_dir_all(&dir)?;
    for n in 1..=1000u32 {
        if n > 1 {
            r.id = format!("{base}-{n}");
        }
        let path = dir.join(format!("{}.json", r.id));
        let json = serde_json::to_string_pretty(r).map_err(io::Error::other)?;
        let tmp = dir.join(format!(".{}.{}.new", r.id, std::process::id()));
        fs::write(&tmp, json)?;
        match fs::hard_link(&tmp, &path) {
            Ok(()) => {
                let _ = fs::remove_file(&tmp);
                return Ok(path);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                let _ = fs::remove_file(&tmp);
            }
            Err(_) if !path.exists() => {
                fs::rename(&tmp, &path)?;
                return Ok(path);
            }
            Err(_) => {
                let _ = fs::remove_file(&tmp);
            }
        }
    }
    Err(io::Error::other("no free name for the report"))
}

pub struct Listing {
    /// Newest first.
    pub reports: Vec<Report>,
    /// Files in the ashtray that are not a report this build can read; left alone.
    pub unreadable: usize,
}

pub fn list(root: &Path) -> io::Result<Listing> {
    let mut reports = Vec::new();
    let mut unreadable = 0;
    for dir in [ashtray_dir(root), crashes_dir(root)] {
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().is_some_and(|e| e == "json")
                && !entry.file_name().to_string_lossy().starts_with('.')
            {
                match fs::read_to_string(&p)
                    .ok()
                    .and_then(|t| serde_json::from_str::<Report>(&t).ok())
                {
                    Some(r) => reports.push(r),
                    None => unreadable += 1,
                }
            }
        }
    }
    reports.sort_by(|a, b| b.id.cmp(&a.id));
    Ok(Listing {
        reports,
        unreadable,
    })
}

pub enum Found {
    One(Box<Report>),
    None,
    Many(usize),
}

pub fn find(root: &Path, id: &str) -> io::Result<Found> {
    let all = list(root)?.reports;
    if let Some(r) = all.iter().find(|r| r.id == id) {
        return Ok(Found::One(Box::new(r.clone())));
    }
    let m: Vec<&Report> = all
        .iter()
        .filter(|r| !id.is_empty() && r.id.starts_with(id))
        .collect();
    Ok(match m.len() {
        0 => Found::None,
        1 => Found::One(Box::new(m[0].clone())),
        n => Found::Many(n),
    })
}

// ----- consent ----------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Consent {
    Ask,
    Always,
    Never,
}

/// Where the answer came from, for the message that says why.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// `CIG_REPORTS`, cig's own variable: one setting for the family.
    CigReports,
    /// `CIGI_REPORTS` or the `reports` line of `installer/config`.
    Setting,
    /// The machine layer locks `reports` (#17): nothing a person sets changes it.
    Locked,
    /// Nothing was set and this is CI with no terminal: never.
    Ci,
    Default,
}

fn parse(v: &str) -> Consent {
    match v.trim() {
        "always" => Consent::Always,
        "never" => Consent::Never,
        _ => Consent::Ask,
    }
}

pub fn under_ci() -> bool {
    std::env::var("CI").is_ok_and(|v| !matches!(v.as_str(), "" | "0" | "false"))
        && !io::stdin().is_terminal()
}

/// The `reports` setting as cig reads it. `CIG_REPORTS` is cig's variable and wins, so a person's
/// `never` holds for every product; then `CIGI_REPORTS` and the installer's config line; with
/// nothing set, CI means never and a person means ask.
pub fn consent(cfg: &Config) -> (Consent, Source) {
    // a locked `reports` outranks cig's variable too: the organization's word is the last one
    if cfg.locked_by("reports").is_some() {
        return (parse(&cfg.get("reports")), Source::Locked);
    }
    if let Ok(v) = std::env::var("CIG_REPORTS") {
        if !v.trim().is_empty() {
            return (parse(&v), Source::CigReports);
        }
    }
    if cfg.source("reports") != "default" {
        return (parse(&cfg.get("reports")), Source::Setting);
    }
    if under_ci() {
        return (Consent::Never, Source::Ci);
    }
    (Consent::Ask, Source::Default)
}

/// A `never` anywhere is a `never`: nothing leaves, not even for a report typed by name. A
/// `reports` the machine layer locks is the one exception to "anywhere": it is the only word.
pub fn explicit_never(cfg: &Config) -> bool {
    if cfg.locked_by("reports").is_some() {
        // locked: the machine's value is the only one, whatever the environment says
        return cfg.get("reports") == "never";
    }
    std::env::var("CIG_REPORTS").is_ok_and(|v| v.trim() == "never")
        || (cfg.source("reports") != "default" && cfg.get("reports") == "never")
}
