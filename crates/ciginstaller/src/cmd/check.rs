// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0
//! `check [--carton <v>] [--project <dir>]` (#4): the marry-up matrix of `cig-compat`, for this
//! machine's combination, for one carton, and for the pins of the project at hand. Each finding
//! goes through the grammar with its code and its fixes; `--json` carries all of them. Any
//! Refuse exits 2.

use std::collections::BTreeSet;

use cig_compat::{
    check, check_pins, worst, Combination, Finding, Host, Manifest, Origin, Pin, Severity, Version,
};
use cigi_ui::{Icon, InstallerError, Report, EXIT_USAGE};
use serde_json::json;

use super::Outcome;
use crate::cx::{Cx, ManifestMode};

fn severity_word(s: Severity) -> &'static str {
    match s {
        Severity::Refuse => "refuse",
        Severity::Warn => "warn",
        Severity::Note => "note",
    }
}

/// The finding as JSON: `{code, severity, headline, detail, fixes, subject}`.
pub fn finding_json(f: &Finding) -> serde_json::Value {
    json!({
        "code": f.code,
        "severity": severity_word(f.severity),
        "headline": f.headline,
        "detail": f.detail,
        "fixes": f.fixes,
        "subject": f.subject,
    })
}

fn host_with_kernel(cx: &Cx) -> Host {
    let mut h = cx.host.clone();
    if cfg!(target_os = "linux") {
        h.kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
    }
    h
}

/// The manifest from the cache; when there is none and the network is allowed, a fetch.
fn manifest(cx: &Cx) -> Result<&Manifest, InstallerError> {
    match cx.manifest(ManifestMode::CacheOnly) {
        Ok(m) => Ok(m),
        Err(_) if !cx.globals.offline => cx.manifest(ManifestMode::Daily),
        Err(e) => Err(e),
    }
}

fn parse_versions(tools: &std::collections::BTreeMap<String, String>) -> Vec<(String, Version)> {
    tools
        .iter()
        .filter_map(|(t, v)| Version::parse(v).ok().map(|v| (t.clone(), v)))
        .collect()
}

pub fn run(cx: &mut Cx, carton: Option<&str>) -> Result<Outcome, InstallerError> {
    let m: Manifest = manifest(cx)?.clone();
    let host = host_with_kernel(cx);
    let mut findings: Vec<Finding> = Vec::new();
    let scope: String;

    if let Some(want) = carton {
        let pin = Pin::parse(want).map_err(|e| {
            InstallerError::new("E1870", e.to_string())
                .with_fix("ciginstaller check --carton 0.1.0")
                .with_exit(EXIT_USAGE)
        })?;
        match m.newest_matching(&pin, None) {
            Some(c) => {
                scope = format!("carton {}", c.version);
                let items = c
                    .products
                    .iter()
                    .filter(|(t, _)| t.as_str() != "ciginstaller")
                    .map(|(t, v)| (t.clone(), v.clone()))
                    .collect();
                findings.extend(check(
                    &m,
                    &Combination {
                        items,
                        origin: Origin::Carton(c.version.clone()),
                    },
                    &host,
                ));
            }
            None => {
                // the engine words this refusal (E1811) for an exact version
                let v = Version::parse(want).map_err(|e| {
                    InstallerError::new("E1870", e.to_string())
                        .with_fix("ciginstaller carton list")
                        .with_exit(EXIT_USAGE)
                })?;
                scope = format!("carton {v}");
                findings.extend(check(
                    &m,
                    &Combination {
                        items: Vec::new(),
                        origin: Origin::Carton(v),
                    },
                    &host,
                ));
            }
        }
    } else {
        match cx
            .state
            .machine_carton
            .as_deref()
            .and_then(|c| Version::parse(c).ok().map(|v| (c.to_string(), v)))
        {
            None => {
                return Ok(Report::new(
                    Icon::Note,
                    "nothing is installed, so there is nothing to check",
                )
                .next("ciginstaller install carton")
                .data(json!({ "scope": "machine", "findings": [] }))
                .into())
            }
            Some((name, cv)) => {
                scope = format!("this machine (carton {name})");
                let mut items = cx
                    .state
                    .cartons
                    .get(&name)
                    .map(|r| parse_versions(&r.tools))
                    .unwrap_or_default();
                for s in &cx.state.sticks {
                    if let Ok(v) = Version::parse(&s.version) {
                        items.retain(|(t, _)| *t != s.tool);
                        items.push((s.tool.clone(), v));
                    }
                }
                let origin = if cx.state.sticks.is_empty() {
                    Origin::Carton(cv)
                } else {
                    Origin::Stick { carton: cv }
                };
                findings.extend(check(&m, &Combination { items, origin }, &host));
            }
        }
        findings.extend(project_findings(cx, &m, &host)?);
    }

    let mut seen = BTreeSet::new();
    findings.retain(|f| seen.insert((f.code, f.subject.clone(), f.headline.clone())));
    findings
        .sort_by(|a, b| (a.severity, a.code, &a.subject).cmp(&(b.severity, b.code, &b.subject)));
    for f in &findings {
        cx.ui.finding(f);
    }
    let refused = findings
        .iter()
        .filter(|f| f.severity == Severity::Refuse)
        .count();
    let warned = findings
        .iter()
        .filter(|f| f.severity == Severity::Warn)
        .count();
    let noted = findings.len() - refused - warned;
    let data = json!({
        "scope": scope,
        "worst": worst(&findings).map(severity_word),
        "findings": findings.iter().map(finding_json).collect::<Vec<_>>(),
    });
    let report = if findings.is_empty() {
        Report::done(format!("{scope}: every combination checks out"))
    } else if refused > 0 {
        Report::new(
            Icon::Refused,
            format!("{scope}: {refused} refused, {warned} warning(s), {noted} note(s)"),
        )
    } else {
        Report::done(format!(
            "{scope}: nothing refused; {warned} warning(s), {noted} note(s)"
        ))
    };
    Ok(report.data(data).into())
}

/// The pins of the project at hand against what is installed (SPEC 8, step 2 and 3).
fn project_findings(cx: &Cx, m: &Manifest, host: &Host) -> Result<Vec<Finding>, InstallerError> {
    let Some(ctx) = cx.project()? else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    if let Some((pin, file)) = &ctx.carton_pin {
        let installed: Vec<Version> = cx
            .state
            .cartons
            .keys()
            .filter_map(|c| Version::parse(c).ok())
            .collect();
        match m.newest_matching(pin, Some(&installed)) {
            Some(c) => {
                let items = cx
                    .state
                    .cartons
                    .get(&c.version.to_string())
                    .map(|r| parse_versions(&r.tools))
                    .unwrap_or_default();
                out.extend(check(
                    m,
                    &Combination {
                        items,
                        origin: Origin::Carton(c.version.clone()),
                    },
                    host,
                ));
            }
            None => {
                let (code, severity, what, fix) = match m.newest_matching(pin, None) {
                    Some(c) => (
                        "E1816",
                        Severity::Note,
                        format!(
                            "this project pins carton {pin} ({}); carton {} is not installed",
                            crate::project::file_label(file),
                            c.version
                        ),
                        format!(
                            "install it: ciginstaller install carton --carton {}",
                            c.version
                        ),
                    ),
                    None => (
                        "E1811",
                        Severity::Refuse,
                        format!(
                            "this project pins carton {pin} ({}); no such carton is on channel {}",
                            crate::project::file_label(file),
                            m.channel
                        ),
                        "see what exists: ciginstaller carton list".to_string(),
                    ),
                };
                out.push(Finding {
                    code,
                    severity,
                    headline: what,
                    detail: Vec::new(),
                    fixes: vec![fix],
                    subject: format!("carton {pin}"),
                });
            }
        }
    }
    if !ctx.legacy.is_empty() {
        let pins: Vec<(String, Pin)> = ctx
            .legacy
            .iter()
            .map(|(t, p, _)| (t.clone(), p.clone()))
            .collect();
        let mut have: Vec<(String, Version)> = Vec::new();
        for r in cx.state.cartons.values() {
            have.extend(parse_versions(&r.tools));
        }
        for s in &cx.state.sticks {
            if let Ok(v) = Version::parse(&s.version) {
                have.push((s.tool.clone(), v));
            }
        }
        have.sort();
        have.dedup();
        out.extend(check_pins(m, &pins, &have, host));
    }
    Ok(out)
}
