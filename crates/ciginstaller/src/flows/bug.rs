// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `bug` (#29 step 1): what a person's bug report carries, and what it looks like before it is
//! saved. The questions and the saving are `cmd/bug.rs`; this is the part that has no terminal in
//! it, so what the preview shows and what is saved are provably one object.
//!
//! A bug report is a crash report of kind `report` (origin `ciginstaller`) whose `details` say
//! `ciginstaller-bug`. Its sections, each removable except the version matrix:
//!
//! | section | holds |
//! |---|---|
//! | `answers` | what the person did, expected and saw, in their words |
//! | `version-matrix` | report's version block, unchanged |
//! | `environment` | the carton and channel, the OS |
//! | `doctor` | `doctor`'s findings that are not ok, for the product the report is about |
//! | `run` | the installer's record of an operation, or cig's bundle for a run |
//! | `repro` | the verified repro, when the run's bundle carries one |
//!
//! Nothing here opens a socket or writes the ashtray.

use std::collections::BTreeSet;
use std::path::Path;

use cigi_ui::{InstallerError, EXIT_USAGE};
use serde_json::{json, Map, Value};

use super::doctor::{self, DoctorArgs};
use super::report as flow;
use crate::cmd::report::gather_run;
use crate::crash::{self, anon, route, Kind, ORIGIN};
use crate::cx::Cx;

/// The parts of the family a bug can be about: the word, how it reads, the key that picks it.
pub const PARTS: &[(&str, &str, char)] = &[
    ("cig", "cig", 'c'),
    ("cigo", "cigo", 'o'),
    ("cigcompile", "CigCompile", 'm'),
    ("program", "a compiled program", 'p'),
    ("cigar", "CigarScript", 'a'),
    ("installer", "the installer", 'i'),
    ("unsure", "not sure", 'n'),
];

/// The sections of a report, by the word `--without` takes.
pub const SECTIONS: &[&str] = &[
    "answers",
    "version-matrix",
    "environment",
    "doctor",
    "run",
    "repro",
];

/// What a bug report's `details.kind` says.
pub const DETAILS_KIND: &str = "ciginstaller-bug";

fn usage(msg: impl Into<String>) -> InstallerError {
    InstallerError::new("E1870", msg).with_exit(EXIT_USAGE)
}

/// A part, from a word a person might type (`cigcompile`, `CigCompile`, `cig-compile`, a
/// program, the installer, `unsure`).
pub fn part_of(word: &str) -> Result<&'static str, InstallerError> {
    let w: String = word
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    let w = match w.as_str() {
        "cigorchestrator" => "cigo",
        "cigscript" => "cig",
        "cigarscript" => "cigar",
        "ciginstaller" => "installer",
        "compiledprogram" | "compiled" => "program",
        "notsure" => "unsure",
        other => other,
    };
    PARTS
        .iter()
        .find(|(p, _, _)| *p == w)
        .map(|(p, _, _)| *p)
        .ok_or_else(|| {
            usage(format!(
                "`{word}` is not a part of the family; the parts are cig, cigo, cigcompile, program, cigar, installer and unsure"
            ))
        })
}

pub fn label_of(part: &str) -> &'static str {
    PARTS
        .iter()
        .find(|(p, _, _)| *p == part)
        .map_or("not sure", |(_, l, _)| *l)
}

/// The repository a part's bugs belong in. Where the part is not known it is the installer's.
pub fn repo_of(part: &str) -> &'static str {
    crash::repo_of_origin(match part {
        "cig" | "program" => "cig",
        "cigo" => "cigo",
        "cigcompile" => "cigcompile",
        "cigar" => "cigar",
        _ => ORIGIN,
    })
}

/// The sections a `--without` list names. The version matrix cannot be removed: without it
/// nobody can act on the report.
pub fn removal(words: &[String]) -> Result<BTreeSet<&'static str>, InstallerError> {
    let mut out = BTreeSet::new();
    for w in words {
        let w = w.trim().to_ascii_lowercase().replace(['_', ' '], "-");
        let w = if w == "matrix" || w == "versions" {
            "version-matrix".to_string()
        } else {
            w
        };
        match SECTIONS.iter().find(|s| **s == w) {
            Some(&"version-matrix") => {
                return Err(usage(
                    "the version matrix cannot be removed: without it nobody can act on the report",
                )
                .with_fix("ciginstaller bug --without doctor"))
            }
            Some(s) => {
                out.insert(*s);
            }
            None => {
                return Err(usage(format!(
                    "`{w}` is not a section; the sections are {}",
                    SECTIONS.join(", ")
                )))
            }
        }
    }
    Ok(out)
}

/// An error code in a run's words (`E704`), as it is written: `E` and three or four digits.
pub fn code_in(text: &str) -> Option<String> {
    let b = text.as_bytes();
    (0..b.len()).find_map(|i| {
        if b[i] != b'E' || (i > 0 && b[i - 1].is_ascii_alphanumeric()) {
            return None;
        }
        let n = b[i + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
        let after = b.get(i + 1 + n).is_some_and(u8::is_ascii_alphanumeric);
        ((3..=4).contains(&n) && !after).then(|| text[i..i + 1 + n].to_string())
    })
}

/// The part an error code or a run points at: E1000-E1099 is CigCompile and its programs,
/// E1800-E1899 the installer, E870-E899 cigo's guard, any other code or a run of cig's is cig.
pub fn suggest_part(code: Option<&str>, operation: bool, cig_run: bool) -> Option<&'static str> {
    if operation {
        return Some("installer");
    }
    if let Some(n) = code
        .and_then(|c| c.strip_prefix('E'))
        .and_then(|d| d.parse::<u32>().ok())
    {
        return Some(match n {
            1000..=1099 => "cigcompile",
            1800..=1999 => "installer",
            870..=899 => "cigo",
            _ => "cig",
        });
    }
    cig_run.then_some("cig")
}

// ------------------------------------------------------------------------- the evidence

/// What a run's bundle said about reproducing it.
pub enum Repro {
    /// Built and verified on this machine; attached.
    Verified(Value),
    /// A repro was built and did not reproduce on this machine: not attached.
    NotVerified,
    /// The run's bundle carries none (or there is no run).
    Missing(&'static str),
}

impl Repro {
    pub fn status(&self) -> String {
        match self {
            Repro::Verified(_) => "verified on this machine".into(),
            Repro::NotVerified => {
                "a repro was built from the run and did not reproduce on this machine".into()
            }
            Repro::Missing(why) => format!("no repro: {why}"),
        }
    }
    pub fn verified(&self) -> bool {
        matches!(self, Repro::Verified(_))
    }
}

/// The run a report is about, when it is about one.
pub struct RunEvidence {
    pub id: String,
    /// `installer` (an operation of this machine) or `cig` (a run of cig's).
    pub source: &'static str,
    pub value: Value,
}

pub struct Evidence {
    pub matrix: Value,
    pub environment: Value,
    pub doctor: Value,
    pub run: Option<RunEvidence>,
    pub repro: Repro,
    /// The error code in the run, for blame and for the fingerprint.
    pub code: Option<String>,
}

/// The newest thing that failed on this machine: a failed operation of the installer or a run
/// of cig's that did not end ok, whichever is later.
pub fn last_failed(cx: &Cx) -> Option<(String, String, String)> {
    let op = cx
        .state
        .history
        .iter()
        .filter(|h| !h.ok)
        .max_by(|a, b| a.at.cmp(&b.at))
        .map(|h| {
            (
                if h.txn.is_empty() {
                    h.id.clone()
                } else {
                    h.txn.clone()
                },
                h.at.clone(),
                format!("{} failed", h.what),
            )
        });
    let cig = cig_runs(cx)
        .into_iter()
        .filter(|r| !matches!(r["status"].as_str(), Some("ok" | "running") | None))
        .max_by(|a, b| a["started"].as_str().cmp(&b["started"].as_str()))
        .and_then(|r| {
            Some((
                r["id"].as_str()?.to_string(),
                r["started"].as_str().unwrap_or("").to_string(),
                r["status"].as_str().unwrap_or("failed").to_string(),
            ))
        });
    match (op, cig) {
        (Some(o), Some(c)) => Some(if o.1 >= c.1 { o } else { c }),
        (o, c) => o.or(c),
    }
}

/// cig's own list of its runs, when cig is installed and answers; a suggestion only, so any
/// trouble is an empty list.
fn cig_runs(cx: &Cx) -> Vec<Value> {
    let Some(ver) = super::family::installed_version(&cx.state, "cig") else {
        return Vec::new();
    };
    let Some(exe) = super::tool_exe(&cx.home, "cig", &ver) else {
        return Vec::new();
    };
    if !cigi_home::verify_toolchain(&cx.home, "cig", &ver).is_empty() {
        return Vec::new();
    }
    let (bin, root) = (cx.home.bin(), cx.home.root());
    doctor::run_args(&exe, &["--json", "runs"], &root, 20, Some(&bin))
        .ok()
        .filter(|r| r.code == Some(0))
        .and_then(|r| serde_json::from_str::<Vec<Value>>(r.stdout.trim()).ok())
        .unwrap_or_default()
}

fn words_of(s: &str) -> impl Iterator<Item = &str> {
    s.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
}

/// `doctor`'s findings that are not ok, for the product the report is about: those that name
/// it, with the count of the others. The installer, or no part, takes every one.
pub fn doctor_for(cx: &mut Cx, part: &str) -> Value {
    let product = match part {
        "cig" | "program" => Some("cig"),
        "cigo" => Some("cigo"),
        "cigcompile" => Some("cigcompile"),
        "cigar" => Some("cigar"),
        _ => None,
    };
    let items = doctor::findings(cx, &DoctorArgs::default());
    let (mut mine, mut others, mut ok) = (Vec::new(), 0usize, 0usize);
    for i in &items {
        let sev = format!("{:?}", i.sev).to_ascii_lowercase();
        if sev == "ok" {
            ok += 1;
            continue;
        }
        let about = product.is_none_or(|p| {
            words_of(&format!("{} {} {}", i.section, i.found, i.because))
                .any(|w| w.eq_ignore_ascii_case(p))
        });
        if about {
            mine.push(json!({
                "section": i.section, "severity": sev, "found": i.found,
                "because": i.because, "fix": i.fix, "code": i.code,
            }));
        } else {
            others += 1;
        }
    }
    json!({ "for": product.unwrap_or("everything"), "findings": mine, "other_findings": others, "ok": ok })
}

/// What a report carries besides the person's words. `cx` is `None` when the installer's state
/// cannot be read: the matrix then says so, and the report still goes.
pub fn gather(
    cx: Option<&mut Cx>,
    broken: Option<&InstallerError>,
    run: Option<&str>,
    part: &str,
    root: &Path,
) -> Result<Evidence, InstallerError> {
    let home = Some(root);
    let Some(cx) = cx else {
        let why = broken.map_or_else(
            || "state.json cannot be read".to_string(),
            |e| e.message.clone(),
        );
        let mut matrix = broken.map_or(Value::Null, flow::block_without_state);
        anon::anonymise_value(&mut matrix, home);
        return Ok(Evidence {
            environment: environment(&matrix),
            matrix,
            doctor: json!({ "for": "everything", "findings": [], "note": format!("not run: {why}") }),
            run: None,
            repro: Repro::Missing("there is no run"),
            code: None,
        });
    };
    let mut matrix = flow::version_block(cx);
    anon::anonymise_value(&mut matrix, home);
    let mut doctor = doctor_for(cx, part);
    anon::anonymise_value(&mut doctor, home);
    let (operation, cig, id) = match run {
        Some(r) => gather_run(cx, Some(r), false)?,
        None => (None, None, None),
    };
    let mut cig = cig;
    let repro_value = cig
        .as_mut()
        .and_then(Value::as_object_mut)
        .and_then(|o| o.remove("repro"));
    let repro = match repro_value {
        Some(v) if v["verified"] == true => Repro::Verified(v),
        Some(_) => Repro::NotVerified,
        None if operation.is_some() => {
            Repro::Missing("the installer's operation is described by its journal")
        }
        None if cig.is_some() => Repro::Missing("the run's bundle carries none"),
        None => Repro::Missing("there is no run"),
    };
    let code = cig
        .as_ref()
        .and_then(|b| {
            b["diagnostic"]["code"]
                .as_str()
                .map(str::to_string)
                .or_else(|| b["run"]["error"].as_str().and_then(code_in))
        })
        .or_else(|| {
            operation
                .as_ref()
                .and_then(|o| code_in(&o["journal"].to_string()))
        });
    let run = id.map(|id| {
        let (source, mut value) = match (operation, cig) {
            (Some(op), _) => ("installer", json!({ "operation": op })),
            (None, Some(b)) => ("cig", json!({ "cig_bundle": b })),
            _ => ("cig", Value::Null),
        };
        anon::anonymise_value(&mut value, home);
        RunEvidence { id, source, value }
    });
    Ok(Evidence {
        environment: environment(&matrix),
        matrix,
        doctor,
        run,
        repro,
        code,
    })
}

/// The carton and channel and the OS, read from the matrix.
fn environment(matrix: &Value) -> Value {
    json!({
        "carton": matrix["carton"]["machine"],
        "channel": matrix["carton"]["channel"].as_str().or_else(|| matrix["installer"]["channel"].as_str()),
        "platform": matrix["platform"],
    })
}

// ------------------------------------------------------------------------- the draft

/// What the person said, after every marked span has been decided.
#[derive(Clone, Debug, Default)]
pub struct Answers {
    pub did: String,
    pub expected: String,
    pub happened: String,
    /// `yes`, `no` or `unsure`; only asked when no repro was verified.
    pub every_time: String,
    /// Spans the person saw marked and chose to keep, as typed. They stay on this machine; they
    /// are what lets the preview and the send carry them as they were typed.
    pub kept: Vec<String>,
}

pub struct Draft {
    pub part: String,
    pub answers: Answers,
    pub evidence: Evidence,
    pub removed: BTreeSet<&'static str>,
}

/// The report, as saved. `compose(...)` is the one place it is made: the preview is
/// `route::travel` of it, `bug --save` writes it, and `crash show` prints the same thing from the
/// file.
pub fn compose(d: &Draft, root: &Path) -> crash::Report {
    let ev = &d.evidence;
    let mut details = Map::new();
    details.insert("kind".into(), json!(DETAILS_KIND));
    details.insert("format".into(), json!(1));
    details.insert("installer".into(), json!(env!("CARGO_PKG_VERSION")));
    details.insert("part".into(), json!(d.part));
    if !d.removed.contains("answers") {
        let mut a = Map::new();
        for (k, v) in [
            ("did", &d.answers.did),
            ("expected", &d.answers.expected),
            ("happened", &d.answers.happened),
            ("every_time", &d.answers.every_time),
        ] {
            if !v.is_empty() {
                a.insert(k.into(), json!(v));
            }
        }
        details.insert("answers".into(), Value::Object(a));
    }
    details.insert("version_matrix".into(), ev.matrix.clone());
    if !d.removed.contains("environment") {
        details.insert("environment".into(), ev.environment.clone());
    }
    if !d.removed.contains("doctor") {
        details.insert("doctor".into(), ev.doctor.clone());
    }
    if !d.removed.contains("run") {
        if let Some(r) = &ev.run {
            details.insert(
                "run".into(),
                json!({ "id": r.id, "source": r.source, "record": r.value }),
            );
        }
    }
    if !d.removed.contains("repro") {
        details.insert("repro_status".into(), json!(ev.repro.status()));
    }
    details.insert(
        "removed".into(),
        json!(d.removed.iter().copied().collect::<Vec<_>>()),
    );
    let said = [&d.answers.happened, &d.answers.did, &d.answers.expected]
        .into_iter()
        .find(|s| !s.is_empty())
        .map(|s| s.lines().next().unwrap_or("").to_string())
        .unwrap_or_else(|| "no description".to_string());
    // the title is always the anonymised text: a span the person kept is in the body, not here
    let said = anon::anonymise_text_in(&said, Some(root));
    let message = ev.code.clone().unwrap_or_else(|| "bug report".to_string());
    let mut r = crash::new_report(
        ORIGIN,
        env!("CARGO_PKG_VERSION"),
        Kind::Report,
        &format!("bug/{}", d.part),
        &message,
        Vec::new(),
        Some(root),
    );
    r.title = Some(crash::truncate(
        &format!("bug ({}): {said}", label_of(&d.part)),
        200,
    ));
    r.repo = Some(repo_of(&d.part).to_string());
    r.active_run_id = ev.run.as_ref().map(|x| {
        x.id.chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
            .take(64)
            .collect()
    });
    r.details = Some(Value::Object(details));
    if let (Repro::Verified(x), false) = (&ev.repro, d.removed.contains("repro")) {
        r.repro = Some(x.clone());
    }
    if !d.answers.kept.is_empty() && !d.removed.contains("answers") {
        r.extra.insert("kept_spans".into(), json!(d.answers.kept));
    }
    r
}

/// Exactly what would travel.
pub fn view(r: &crash::Report, root: &Path) -> Value {
    route::travel(r, root)
}

/// The preview as lines: the whole object, exactly as `crash show` prints it and `crash send`
/// sends it, with a `## ` label line in front of each section so it reads section by section.
/// Take the label lines out and what is left is `serde_json::to_string_pretty(view)`, line for
/// line.
pub fn preview_lines(v: &Value) -> Vec<String> {
    const LABELS: &[(&str, &str)] = &[
        (
            "  \"details\":",
            "## about this report, and its sections below",
        ),
        (
            "    \"answers\":",
            "## answers (can be removed: --without answers)",
        ),
        ("    \"version_matrix\":", "## version matrix (always sent)"),
        (
            "    \"environment\":",
            "## environment (can be removed: --without environment)",
        ),
        (
            "    \"doctor\":",
            "## doctor (can be removed: --without doctor)",
        ),
        ("    \"run\":", "## run (can be removed: --without run)"),
        ("  \"repro\":", "## repro (can be removed: --without repro)"),
    ];
    let mut out = Vec::new();
    for line in serde_json::to_string_pretty(v).unwrap_or_default().lines() {
        if let Some((_, label)) = LABELS.iter().find(|(p, _)| line.starts_with(p)) {
            out.push((*label).to_string());
        }
        out.push(line.to_string());
    }
    out
}

/// The bug reports in a listing, newest first.
pub fn is_bug(r: &crash::Report) -> bool {
    r.kind == Kind::Report
        && r.origin == ORIGIN
        && r.details
            .as_ref()
            .is_some_and(|d| d["kind"] == DETAILS_KIND)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev() -> Evidence {
        Evidence {
            matrix: json!({"format": 1, "tools": [], "mismatches": []}),
            environment: json!({"carton": "0.1.0"}),
            doctor: json!({"for": "cig", "findings": []}),
            run: None,
            repro: Repro::Missing("there is no run"),
            code: None,
        }
    }

    fn draft(removed: &[&'static str]) -> Draft {
        Draft {
            part: "cig".into(),
            answers: Answers {
                did: "rolled back a run".into(),
                ..Default::default()
            },
            evidence: ev(),
            removed: removed.iter().copied().collect(),
        }
    }

    #[test]
    fn the_matrix_cannot_be_removed_and_a_typo_is_not_a_section() {
        assert!(removal(&["version-matrix".into()]).is_err());
        assert!(removal(&["matrix".into()]).is_err());
        assert!(removal(&["nonsense".into()]).is_err());
        assert_eq!(removal(&["doctor".into(), "run".into()]).unwrap().len(), 2);
    }

    #[test]
    fn a_removed_section_is_named_and_gone_and_the_matrix_stays() {
        let root = std::env::temp_dir();
        let r = compose(&draft(&["doctor", "answers"]), &root);
        let d = r.details.unwrap();
        assert!(d.get("doctor").is_none() && d.get("answers").is_none());
        assert!(d.get("version_matrix").is_some());
        assert_eq!(d["removed"], json!(["answers", "doctor"]));
    }

    #[test]
    fn codes_blame_the_part_from_the_ranges() {
        assert_eq!(
            suggest_part(Some("E1042"), false, false),
            Some("cigcompile")
        );
        assert_eq!(suggest_part(Some("E875"), false, true), Some("cigo"));
        assert_eq!(suggest_part(Some("E704"), false, true), Some("cig"));
        assert_eq!(suggest_part(Some("E1858"), false, false), Some("installer"));
        assert_eq!(suggest_part(None, true, false), Some("installer"));
        assert_eq!(suggest_part(None, false, false), None);
        assert_eq!(code_in("died with E704 on hop 2").as_deref(), Some("E704"));
        assert_eq!(code_in("SHE7041"), None);
    }

    #[test]
    fn replaced_text_is_stable_under_the_anonymiser() {
        let root = std::env::temp_dir();
        let text = "the file /srv/data/notes/plan.txt and a@b.example";
        let ms = anon::marks(text, Some(&root));
        let once = anon::apply_marks(text, &ms, &|_| false);
        assert_eq!(anon::anonymise_text_in(&once, Some(&root)), once);
    }
}
