// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! Fixed-version announcements across the family (#12).
//!
//! A bug this machine reported against any tool is told to its reporter when a carton carries the
//! fix, in one line:
//!
//! ```text
//! the bug you reported (cig-orchestrator#NN, <title>) is fixed in cigo 1.4.2, which is in
//! carton 1.1.3. update: ciginstaller update
//! ```
//!
//! The fixed version comes from the manifest's advisories (#2): an advisory whose `url` is the
//! report's issue, or whose `id` is `<owner>/<repo>#<n>` or `<repo>#<n>`, with `fixed_in`. The
//! tool is the one whose repository the issue is filed in. cig's own reports are cig's to
//! announce (`cig crash announce-fixed`, after an update that changed cig); every other tool's
//! are the installer's. Each report is announced once: `announced` and `fixed_in` are written
//! into its file in the ashtray.

use std::path::Path;

use cig_compat::{Advisory, Carton, Manifest};

use crate::crash::{self, Issue};

/// One reported bug whose fix a carton carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fix {
    /// The report's id in the ashtray.
    pub report: String,
    /// `cig-orchestrator#12`.
    pub reference: String,
    pub title: String,
    pub tool: String,
    /// The first version with the fix.
    pub version: String,
    pub carton: String,
}

/// Does this advisory name this issue?
fn names(a: &Advisory, issue: &Issue) -> bool {
    let short = issue.repo.rsplit('/').next().unwrap_or(&issue.repo);
    a.url.as_deref() == Some(issue.url.as_str())
        || a.id == format!("{}#{}", issue.repo, issue.number)
        || a.id == format!("{short}#{}", issue.number)
}

/// The filed reports, not yet announced, whose fix `carton` carries: its build of the tool the
/// issue is filed against is at or past the advisory's fixed version.
pub fn fixes(root: &Path, m: &Manifest, carton: &Carton) -> Vec<Fix> {
    let Ok(listing) = crash::list(root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for r in listing.reports {
        let Some(issue) = &r.issue else {
            continue;
        };
        if r.announced {
            continue;
        }
        let Some((tool, _)) = m.tools.iter().find(|(_, t)| t.repo == issue.repo) else {
            continue;
        };
        if tool == "cig" {
            continue;
        }
        let Some(built) = carton.products.get(tool) else {
            continue;
        };
        let Some(fixed) = m
            .advisories
            .iter()
            .filter(|a| names(a, issue))
            .filter_map(|a| a.fixed_in.as_ref())
            .min()
        else {
            continue;
        };
        if built < fixed {
            continue;
        }
        let short = issue.repo.rsplit('/').next().unwrap_or(&issue.repo);
        out.push(Fix {
            report: r.id.clone(),
            reference: format!("{short}#{}", issue.number),
            title: crash::truncate(r.headline(), 60),
            tool: tool.clone(),
            version: fixed.to_string(),
            carton: carton.version.to_string(),
        });
    }
    out
}

/// Remember that these were told, so each is told once.
pub fn mark(root: &Path, fixes: &[Fix]) {
    for f in fixes {
        if let Ok(crash::Found::One(mut r)) = crash::find(root, &f.report) {
            r.announced = true;
            r.fixed_in = Some(f.version.clone());
            let _ = crash::write(root, &r);
        }
    }
}

/// The line for one fix: before an update, the carton to take; after it, what it installed.
pub fn line(f: &Fix, installed: bool) -> String {
    let head = format!(
        "the bug you reported ({}, {}) is fixed in {} {}",
        f.reference, f.title, f.tool, f.version
    );
    if installed {
        format!(
            "{head}, which this update installed with carton {}",
            f.carton
        )
    } else {
        format!(
            "{head}, which is in carton {}. update: ciginstaller update",
            f.carton
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fix() -> Fix {
        Fix {
            report: "r1".into(),
            reference: "cig-orchestrator#12".into(),
            title: "rollback skipped a file after a box apply".into(),
            tool: "cigo".into(),
            version: "1.4.2".into(),
            carton: "1.1.3".into(),
        }
    }

    #[test]
    fn the_line_before_an_update_is_the_issues() {
        assert_eq!(
            line(&fix(), false),
            "the bug you reported (cig-orchestrator#12, rollback skipped a file after a box apply) \
             is fixed in cigo 1.4.2, which is in carton 1.1.3. update: ciginstaller update"
        );
    }

    #[test]
    fn the_line_after_an_update_says_it_is_installed() {
        assert!(line(&fix(), true).ends_with("which this update installed with carton 1.1.3"));
    }

    #[test]
    fn an_advisory_names_an_issue_by_url_or_reference() {
        let issue = Issue {
            repo: "otmof-ops/cig-orchestrator".into(),
            number: 12,
            url: "https://github.com/otmof-ops/cig-orchestrator/issues/12".into(),
        };
        let adv = |id: &str, url: Option<&str>| Advisory {
            id: id.into(),
            affects: Vec::new(),
            fixed_in: None,
            severity: cig_compat::AdvisorySeverity::Bug,
            summary: String::new(),
            url: url.map(str::to_string),
        };
        assert!(names(&adv("otmof-ops/cig-orchestrator#12", None), &issue));
        assert!(names(&adv("cig-orchestrator#12", None), &issue));
        assert!(names(&adv("CIGO-1", Some(&issue.url)), &issue));
        assert!(!names(&adv("cig-orchestrator#13", None), &issue));
        assert!(!names(&adv("cig-script#12", None), &issue));
    }
}
