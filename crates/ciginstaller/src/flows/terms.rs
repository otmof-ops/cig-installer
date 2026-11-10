// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! The license display of #27: the licenses of what is about to be installed are shown once, at
//! the first install, and again only when a license the machine has not seen comes in. What is
//! recorded stays in `state.json` (`terms`); nothing is sent anywhere.
//!
//! The licenses are copyright permissions, not contracts: the notice is acknowledged, never
//! "accepted", and nothing here grants or withholds the right to use the code.
//!
//! This module writes no license terms. It names each license by its SPDX id, points at the
//! SPDX page for it, and at the LICENSE files, which are the terms.

use std::collections::BTreeSet;

use cig_compat::{Manifest, Version};
use cigi_home::now_rfc3339;
use cigi_ui::{InstallerError, Mode, Question, EXIT_REFUSED};
use serde_json::{json, Value};

use crate::cx::Cx;

/// The installer's own license, from the workspace's `license` (an SPDX id).
pub const INSTALLER_LICENSE: &str = env!("CARGO_PKG_LICENSE");

/// The repository's LICENSE file, compiled in: the text `licences ciginstaller --text` prints.
pub const INSTALLER_LICENSE_TEXT: &str = include_str!("../../../../LICENSE");

/// Where the installer's source and LICENSE file are.
pub const INSTALLER_REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

/// The key under which the record lives in `state.json` (unknown keys survive a round trip).
const KEY: &str = "terms";

/// What `licences` and the report say for a build whose manifest states no license.
pub const NOT_STATED: &str = "not stated";

/// `state.json`'s `terms`: what was shown, how it was answered, and when. Local only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TermsRecord {
    /// The carton whose install showed the terms.
    pub carton: String,
    /// Every SPDX expression shown (the installer's and each installed build's), sorted.
    pub licenses: Vec<String>,
    /// sha256 of the installer's LICENSE file as it was when shown.
    pub installer_license_sha256: String,
    /// `acknowledged` (answered yes), `acknowledged-by-flag` (`--yes`), or `acknowledged-without-terminal`
    /// (shown in the report, no one to ask).
    pub how: String,
    pub at: String,
}

/// The record in `state.json`, when there is one that reads.
pub fn record(state: &cigi_home::State) -> Option<TermsRecord> {
    let v = state.extra.get(KEY)?;
    let text = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    Some(TermsRecord {
        carton: text("carton")?,
        licenses: v
            .get("licenses")?
            .as_array()?
            .iter()
            .filter_map(|l| l.as_str().map(str::to_string))
            .collect(),
        installer_license_sha256: text("installer_license_sha256")?,
        how: text("how")?,
        at: text("at")?,
    })
}

impl TermsRecord {
    pub fn to_json(&self) -> Value {
        json!({
            "carton": self.carton, "licenses": self.licenses, "how": self.how, "at": self.at,
            "installer_license_sha256": self.installer_license_sha256,
        })
    }
}

/// sha256 of the LICENSE text this binary carries.
pub fn installer_license_sha256() -> String {
    cigi_source::digest::sha256_hex(INSTALLER_LICENSE_TEXT.as_bytes())
}

/// The SPDX ids inside an expression (`MIT OR Apache-2.0` gives both).
pub fn spdx_ids(expr: &str) -> Vec<String> {
    expr.split(|c: char| c.is_whitespace() || c == '(' || c == ')')
        .filter(|t| !t.is_empty() && !matches!(*t, "AND" | "OR" | "WITH"))
        .map(|t| t.trim_end_matches('+').to_string())
        .collect()
}

/// The SPDX page of each id in an expression.
pub fn spdx_urls(expr: &str) -> Vec<String> {
    spdx_ids(expr)
        .iter()
        .map(|id| format!("https://spdx.org/licenses/{id}.html"))
        .collect()
}

/// What `gate` tells the install's report.
pub struct Noted {
    pub line: String,
    pub data: Value,
}

fn refused() -> InstallerError {
    InstallerError::new(
        "E1876",
        "the license notice was not confirmed, so nothing was installed",
    )
    .with_hint(
        "the licenses still let you use the code; this is only the notice, and it is asked once",
    )
    .with_fix("ciginstaller licences")
    .with_exit(EXIT_REFUSED)
}

/// Show the licenses of this install when the machine has not seen them, ask at a terminal, and
/// put the record in `cx.state` for the install's own state save. `None` when there is nothing
/// new to show. A "no" is E1876 and changes nothing.
pub fn gate(
    cx: &mut Cx,
    m: &Manifest,
    carton: &Version,
    want: &[(String, Version)],
) -> Result<Option<Noted>, InstallerError> {
    let mut shown: Vec<(String, String)> = vec![(
        format!("ciginstaller {}", env!("CARGO_PKG_VERSION")),
        INSTALLER_LICENSE.to_string(),
    )];
    for (t, v) in want {
        let lic = m
            .product(t, v)
            .and_then(|p| p.license.clone())
            .unwrap_or_else(|| NOT_STATED.to_string());
        shown.push((format!("{t} {v}"), lic));
    }
    let ids: BTreeSet<String> = shown
        .iter()
        .filter(|(_, l)| l != NOT_STATED)
        .map(|(_, l)| l.clone())
        .collect();
    let sha = installer_license_sha256();
    let before = record(&cx.state);
    let new_to_this_machine = match &before {
        None => true,
        Some(r) => r.installer_license_sha256 != sha || ids.iter().any(|l| !r.licenses.contains(l)),
    };
    if !new_to_this_machine {
        return Ok(None);
    }

    let how = if (cx.ui.mode != Mode::Json && cx.ui.tty_in && cx.ui.tty_err) || cx.ui.yes {
        let mut q =
            Question::yes_no("have you seen these licenses?", false).summary(if before.is_some() {
                "they changed since you last confirmed; the answer is kept on this machine only"
            } else {
                "shown once; the answer is kept on this machine only"
            });
        for (what, lic) in &shown {
            q = q.detail(&format!("{what}: {lic}"));
        }
        q = q
            .detail("the terms are the LICENSE files; the SPDX page names each license")
            .detail("read them first with `ciginstaller licences`");
        let a = cx.ui.ask(&q);
        if !a.is_yes(&q) {
            return Err(refused());
        }
        if a.how == cigi_ui::How::Flag {
            "acknowledged-by-flag"
        } else {
            "acknowledged"
        }
    } else {
        "acknowledged-without-terminal"
    };

    let rec = TermsRecord {
        carton: carton.to_string(),
        licenses: ids.iter().cloned().collect(),
        installer_license_sha256: sha,
        how: how.to_string(),
        at: now_rfc3339(),
    };
    cx.state.extra.insert(KEY.to_string(), rec.to_json());
    let list: Vec<String> = shown.iter().map(|(w, l)| format!("{w} {l}")).collect();
    let line = if how == "acknowledged-without-terminal" {
        format!(
            "licenses: {}; no terminal to ask on, so they are noted, not confirmed; `ciginstaller licences` has the details",
            list.join(", ")
        )
    } else {
        format!(
            "licenses acknowledged: {}; `ciginstaller licences` lists them any time",
            list.join(", ")
        )
    };
    Ok(Some(Noted {
        line,
        data: rec.to_json(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_come_out_of_an_expression() {
        assert_eq!(
            spdx_ids("MIT OR (Apache-2.0 WITH LLVM-exception)"),
            ["MIT", "Apache-2.0", "LLVM-exception"]
        );
        assert_eq!(spdx_ids("GPL-2.0+"), ["GPL-2.0"]);
        assert_eq!(
            spdx_urls("FSL-1.1-ALv2"),
            ["https://spdx.org/licenses/FSL-1.1-ALv2.html"]
        );
    }

    #[test]
    fn the_embedded_text_is_the_repositorys_license() {
        assert!(INSTALLER_LICENSE_TEXT.contains("Functional Source License"));
        assert_eq!(INSTALLER_LICENSE, "FSL-1.1-ALv2");
    }
}
