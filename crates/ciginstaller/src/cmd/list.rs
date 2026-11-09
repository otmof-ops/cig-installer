// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0
//! `list`: installed cartons, sticks and tools.

use cigi_home::read_receipt;
use cigi_ui::{InstallerError, Report};
use serde_json::json;

use super::Outcome;
use crate::cx::Cx;

pub fn run(cx: &mut Cx) -> Result<Outcome, InstallerError> {
    let s = &cx.state;
    if s.cartons.is_empty() && s.sticks.is_empty() {
        return Ok(Report::done("nothing is installed")
            .next("ciginstaller install carton")
            .data(json!({ "cartons": [], "sticks": [], "machine_carton": null }))
            .into());
    }
    let verbose = cx.ui.verbose;
    let mut r = Report::done(format!(
        "{} carton(s) installed; machine carton {}",
        s.cartons.len(),
        s.machine_carton.as_deref().unwrap_or("none")
    ));
    let mut cartons = Vec::new();
    for (c, rec) in &s.cartons {
        let mark = if s.machine_carton.as_deref() == Some(c.as_str()) {
            " (machine)"
        } else {
            ""
        };
        let partial = if rec.partial { " [partial]" } else { "" };
        let tools: Vec<String> = rec.tools.iter().map(|(t, v)| format!("{t} {v}")).collect();
        r = r.detail(format!("carton {c}{mark}{partial}: {}", tools.join(", ")));
        if verbose {
            for (t, v) in &rec.tools {
                if let Some(rc) = cig_compat::Version::parse(v)
                    .ok()
                    .and_then(|v| read_receipt(&cx.home, t, &v).ok())
                {
                    let how = if rc.trust.independent {
                        "independent"
                    } else {
                        "single source"
                    };
                    r = r.detail(format!(
                        "  {t} {v}: {}, {how}, {} files",
                        rc.trust.level,
                        rc.files.len()
                    ));
                }
            }
        }
        cartons.push(json!({ "carton": c, "machine": !mark.is_empty(), "partial": rec.partial, "tools": rec.tools }));
    }
    for st in &s.sticks {
        r = r.detail(format!(
            "stick {} {} (from carton {})",
            st.tool, st.version, st.from_carton
        ));
    }
    Ok(r.data(
        json!({ "machine_carton": s.machine_carton, "cartons": cartons, "sticks": s.sticks }),
    )
    .into())
}
