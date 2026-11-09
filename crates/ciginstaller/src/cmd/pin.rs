// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0
//! `pin carton <pin>`, `pin cig <pin>`; anything else has no pin file (E1843).

use std::path::Path;

use cig_compat::{Pin, Version};
use cigi_ui::{InstallerError, Report, EXIT_REFUSED, EXIT_USAGE};
use serde_json::json;

use super::Outcome;
use crate::cx::Cx;
use crate::errors::ie;
use crate::project::project_root;

fn usage(msg: String, fix: &str) -> InstallerError {
    InstallerError::new("E1870", msg)
        .with_fix(fix)
        .with_exit(EXIT_USAGE)
}

fn write_atomic(path: &Path, text: &str) -> Result<(), InstallerError> {
    let tmp = path.with_file_name(format!(
        ".{}.tmp-{}",
        crate::project::file_label(path),
        std::process::id()
    ));
    std::fs::write(&tmp, text)
        .and_then(|()| std::fs::rename(&tmp, path))
        .map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            InstallerError::new("E1835", format!("cannot write {}: {e}", path.display()))
        })
}

/// Put `carton = "<pin>"` among the top-level keys of a TOML file.
fn toml_with_carton(text: &str, pin: &str) -> String {
    let line = format!("carton = \"{pin}\"");
    let mut out: Vec<String> = Vec::new();
    let mut done = false;
    let mut in_table = false;
    for l in text.lines() {
        let t = l.trim();
        if t.starts_with('[') && !in_table {
            in_table = true;
            if !done {
                out.push(line.clone());
                out.push(String::new());
                done = true;
            }
        }
        if !in_table
            && !done
            && t.strip_prefix("carton")
                .is_some_and(|r| r.trim_start().starts_with('='))
        {
            out.push(line.clone());
            done = true;
            continue;
        }
        out.push(l.to_string());
    }
    if !done {
        out.push(line);
    }
    out.join("\n") + "\n"
}

pub fn run(
    cx: &mut Cx,
    tool: &str,
    pin: &str,
    file: Option<&str>,
) -> Result<Outcome, InstallerError> {
    // pin files live in the project, but the home's permissions are still checked before any write
    cx.home.check_permissions().map_err(ie)?;
    if tool != "carton" && tool != "cig" {
        return Err(InstallerError::new(
            "E1843",
            format!("there is no per-tool pin file for `{tool}`; pin the carton that holds the build you want"),
        )
        .with_fix(format!("ciginstaller pin carton <pin>  # the carton that holds {tool}"))
        .with_exit(EXIT_REFUSED));
    }
    let parsed =
        Pin::parse(pin).map_err(|e| usage(e.to_string(), "ciginstaller pin carton 0.1"))?;
    let root = project_root(&cx.cwd);
    let (path, what) = if tool == "cig" {
        let p = root.join(".cig-version");
        write_atomic(&p, &format!("{parsed}\n"))?;
        (p, "cig")
    } else {
        match file {
            None => {
                let p = root.join(".cig-carton");
                write_atomic(&p, &format!("carton {parsed}\n"))?;
                (p, "carton")
            }
            Some("cig-tasks.json") => {
                let p = root.join("cig-tasks.json");
                let mut v: serde_json::Value = match std::fs::read_to_string(&p) {
                    Ok(t) => serde_json::from_str(&t).map_err(|e| {
                        InstallerError::new(
                            "E1841",
                            format!("{} is not valid JSON: {e}", p.display()),
                        )
                    })?,
                    Err(_) => json!({}),
                };
                let obj = v.as_object_mut().ok_or_else(|| {
                    InstallerError::new("E1841", format!("{} is not a JSON object", p.display()))
                })?;
                obj.insert("carton".into(), json!(parsed.to_string()));
                let mut text = serde_json::to_string_pretty(&v).unwrap_or_default();
                text.push('\n');
                write_atomic(&p, &text)?;
                (p, "carton")
            }
            Some("cigar.toml") => {
                let p = root.join("cigar.toml");
                let old = std::fs::read_to_string(&p).unwrap_or_default();
                write_atomic(&p, &toml_with_carton(&old, &parsed.to_string()))?;
                (p, "carton")
            }
            Some(other) => {
                return Err(usage(
                    format!("`--file {other}` is not cig-tasks.json or cigar.toml"),
                    "ciginstaller pin carton 0.1 --file cig-tasks.json",
                ))
            }
        }
    };
    let mut r = Report::done(format!("{what} pinned to {parsed} in {}", path.display()));
    if what == "carton" {
        let fits = cx
            .state
            .cartons
            .keys()
            .filter_map(|k| Version::parse(k).ok())
            .any(|c| parsed.satisfied_by(&c));
        if !fits {
            r = r.detail(format!(
                "no installed carton fits {parsed} yet; the first run fetches one (pin_fetch)"
            ));
        }
    }
    Ok(
        r.data(json!({ "pin": parsed.to_string(), "file": path.display().to_string() }))
            .into(),
    )
}
