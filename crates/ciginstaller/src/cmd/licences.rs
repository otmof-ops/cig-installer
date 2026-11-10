// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `licences [tool] [--text]` (#27): the license of the installer and of each build on this
//! machine, where its text is, and what was acknowledged. Read-only and offline: nothing here
//! touches the network, and nothing is sent anywhere.
//!
//! The license of a build is the one its receipt recorded at install, from the manifest's
//! `license` field. A build whose manifest stated none shows "not stated". The terms themselves
//! are the LICENSE files; this command names them and prints one on request.

use std::collections::BTreeMap;

use cig_compat::Version;
use cigi_home::{read_receipt, Receipt};
use cigi_ui::{Align, Cell, InstallerError, Report, Table, EXIT_REFUSED, EXIT_USAGE};
use serde_json::json;

use super::tabular::details;
use super::Outcome;
use crate::cx::Cx;
use crate::flows::terms::{
    installer_license_sha256, record, spdx_urls, INSTALLER_LICENSE, INSTALLER_LICENSE_TEXT,
    INSTALLER_REPOSITORY, NOT_STATED,
};

/// The installer's name in this command (it is not a tool of any carton).
const INSTALLER: &str = "ciginstaller";

struct Build {
    tool: String,
    version: Version,
    /// Where it is installed from: `carton 0.1.0`, `stick`.
    from: Vec<String>,
    license: Option<String>,
    receipt: Option<Receipt>,
}

/// A path to a license file inside the toolchain, when the receipt lists one that is there.
fn text_file(cx: &Cx, b: &Build) -> Option<std::path::PathBuf> {
    let r = b.receipt.as_ref()?;
    let dir = cx.home.toolchain(&b.tool, &b.version);
    r.files.iter().find_map(|f| {
        let name = f.path.rsplit('/').next().unwrap_or("").to_ascii_uppercase();
        let is_license = ["LICENSE", "LICENCE", "COPYING", "UNLICENSE"]
            .iter()
            .any(|p| name.starts_with(p));
        if !is_license {
            return None;
        }
        let mut p = dir.clone();
        for part in f.path.split('/') {
            p.push(part);
        }
        p.is_file().then_some(p)
    })
}

fn builds(cx: &Cx) -> Vec<Build> {
    let mut seen: BTreeMap<(String, Version), Vec<String>> = BTreeMap::new();
    for (c, rec) in &cx.state.cartons {
        for (t, v) in &rec.tools {
            if let Ok(v) = Version::parse(v) {
                seen.entry((t.clone(), v))
                    .or_default()
                    .push(format!("carton {c}"));
            }
        }
    }
    for s in &cx.state.sticks {
        if let Ok(v) = Version::parse(&s.version) {
            seen.entry((s.tool.clone(), v))
                .or_default()
                .push("stick".to_string());
        }
    }
    seen.into_iter()
        .filter(|((t, _), _)| t != INSTALLER)
        .map(|((tool, version), from)| {
            let receipt = read_receipt(&cx.home, &tool, &version).ok();
            let license = receipt.as_ref().and_then(|r| r.license.clone());
            Build {
                tool,
                version,
                from,
                license,
                receipt,
            }
        })
        .collect()
}

fn usage(msg: impl Into<String>, fix: &str) -> InstallerError {
    InstallerError::new("E1870", msg)
        .with_fix(fix)
        .with_exit(EXIT_USAGE)
}

pub fn run(cx: &mut Cx, tool: Option<&str>, text: bool) -> Result<Outcome, InstallerError> {
    if text && tool.is_none() {
        return Err(usage(
            "--text prints one license, so it needs a tool: ciginstaller or one of the builds",
            "ciginstaller licences ciginstaller --text",
        ));
    }
    let all = builds(cx);
    if let Some(t) = tool {
        if t != INSTALLER && !all.iter().any(|b| b.tool == t) {
            return Err(InstallerError::new(
                "E1877",
                format!("`{t}` has no build on this machine to list a license for"),
            )
            .with_hint(
                "the list holds the installer and every tool in an installed carton or stick",
            )
            .with_fix("ciginstaller licences")
            .with_exit(EXIT_REFUSED));
        }
    }
    if text {
        return print_text(cx, tool.unwrap_or(INSTALLER), &all);
    }

    let mut table = Table::new(&[
        ("build", Align::Left),
        ("license", Align::Left),
        ("text", Align::Left),
    ]);
    let mut rows = Vec::new();
    let mut shown_installer = false;
    if tool.is_none() || tool == Some(INSTALLER) {
        shown_installer = true;
        table = table.row(vec![
            Cell::new(format!("{INSTALLER} {}", env!("CARGO_PKG_VERSION"))),
            Cell::new(INSTALLER_LICENSE),
            Cell::new("ciginstaller licences ciginstaller --text"),
        ]);
    }
    let mut build_rows = Vec::new();
    for b in all.iter().filter(|b| tool.is_none_or(|t| b.tool == t)) {
        let file = text_file(cx, b);
        let lic = b.license.clone();
        let where_text = match (&file, &b.receipt) {
            (Some(p), _) => p.display().to_string(),
            (None, Some(r)) => format!(
                "not in the install; see {} at {}",
                r.source.repo, r.source.tag
            ),
            (None, None) => "no receipt: ciginstaller repair".to_string(),
        };
        table = table.row(vec![
            Cell::new(format!("{} {}", b.tool, b.version)),
            match &lic {
                Some(l) => Cell::new(l.clone()),
                None => Cell::changed(NOT_STATED),
            },
            Cell::new(where_text),
        ]);
        build_rows.push(json!({
            "tool": b.tool,
            "version": b.version.to_string(),
            "from": b.from,
            "license": lic,
            "spdx_urls": lic.as_deref().map(spdx_urls).unwrap_or_default(),
            "text_path": file.map(|p| p.display().to_string()),
            "source": b.receipt.as_ref().map(|r| json!({ "repo": r.source.repo, "tag": r.source.tag })),
        }));
    }
    if shown_installer {
        rows.push(json!({
            "tool": INSTALLER,
            "version": env!("CARGO_PKG_VERSION"),
            "license": INSTALLER_LICENSE,
            "spdx_urls": spdx_urls(INSTALLER_LICENSE),
            "text_sha256": installer_license_sha256(),
            "repository": INSTALLER_REPOSITORY,
        }));
    }
    let terms = record(&cx.state);
    let n = build_rows.len();
    let mut r = Report::done(format!(
        "{} on this machine",
        match (shown_installer, n) {
            (true, 0) => "the installer's license".to_string(),
            (_, 1) => "1 build's license".to_string(),
            (_, n) => format!("{n} builds' licenses"),
        }
    ));
    for l in details(&cx.ui, &table) {
        r = r.detail(l);
    }
    let unstated = build_rows.iter().filter(|b| b["license"].is_null()).count();
    if unstated > 0 {
        r = r.detail(format!(
            "{unstated} build(s) show \"{NOT_STATED}\": their manifest named no license when they were installed"
        ));
    }
    match &terms {
        Some(t) => {
            r = r.detail(format!(
                "terms: {} with carton {} on {} (kept on this machine only)",
                t.how.replace('-', " "),
                t.carton,
                t.at
            ));
        }
        None => {
            r = r.detail("terms: not shown yet; they are shown once, at the first install");
        }
    }
    r = r.detail(format!(
        "the installer's LICENSE file is at {INSTALLER_REPOSITORY}; the SPDX page for a license is https://spdx.org/licenses/<id>.html"
    ));
    if n > 0 {
        r = r.next("ciginstaller licences <tool> --text");
    } else {
        r = r.next("ciginstaller licences ciginstaller --text");
    }
    Ok(r.data(json!({
        "installer": rows.into_iter().next(),
        "builds": build_rows,
        "terms": terms.as_ref().map(crate::flows::terms::TermsRecord::to_json),
    }))
    .into())
}

fn print_text(cx: &Cx, tool: &str, all: &[Build]) -> Result<Outcome, InstallerError> {
    if tool == INSTALLER {
        return Ok(Outcome::Page(INSTALLER_LICENSE_TEXT.to_string()));
    }
    let newest = all
        .iter()
        .filter(|b| b.tool == tool)
        .max_by(|a, b| a.version.cmp(&b.version));
    let b = newest.ok_or_else(|| {
        usage(
            format!("`{tool}` has no build on this machine"),
            "ciginstaller licences",
        )
    })?;
    let path = text_file(cx, b);
    match path.and_then(|p| std::fs::read_to_string(p).ok()) {
        Some(t) => Ok(Outcome::Page(t)),
        None => Err(InstallerError::new(
            "E1878",
            format!(
                "{} {} carries no license file on this machine",
                b.tool, b.version
            ),
        )
        .with_hint(match &b.receipt {
            Some(r) => format!(
                "its license text is in {} at {}",
                r.source.repo, r.source.tag
            ),
            None => "it has no receipt either".to_string(),
        })
        .with_fix("ciginstaller licences")
        .with_exit(EXIT_REFUSED)),
    }
}
