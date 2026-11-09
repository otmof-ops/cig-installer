// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0
//! `carton`, `carton list`, `carton show <v>`, `carton <v>`.

use cig_compat::{Pin, Version, Yankable};
use cigi_ui::{InstallerError, Report, EXIT_REFUSED, EXIT_USAGE};
use serde_json::json;

use super::Outcome;
use crate::cli::{CartonCmd, CartonSub};
use crate::cx::{Cx, ManifestMode};
use crate::flows::install::{self, InstallArgs};
use crate::flows::use_carton::{self, UseArgs};

pub fn run(cx: &mut Cx, c: &CartonCmd) -> Result<Outcome, InstallerError> {
    match (&c.sub, &c.version) {
        (Some(CartonSub::List { all }), _) => list(cx, *all),
        (Some(CartonSub::Show { version }), _) => show(cx, version),
        (None, Some(v)) => switch(cx, v),
        (None, None) => status(cx),
    }
}

fn status(cx: &mut Cx) -> Result<Outcome, InstallerError> {
    let Some(machine) = cx.state.machine_carton.clone() else {
        return Ok(Report::done("no carton is installed on this machine")
            .next("ciginstaller install carton")
            .data(json!({ "machine_carton": null }))
            .into());
    };
    let newest = cx
        .manifest(ManifestMode::CacheOnly)
        .ok()
        .and_then(|m| m.latest(false).map(|c| c.version.clone()));
    let have = Version::parse(&machine).ok();
    let mut r = Report::done(format!("this machine is on carton {machine}"));
    if let Some(rec) = cx.state.cartons.get(&machine) {
        let t: Vec<String> = rec.tools.iter().map(|(t, v)| format!("{t} {v}")).collect();
        r = r.detail(t.join(", "));
    }
    let mut newer = None;
    if let (Some(n), Some(h)) = (&newest, &have) {
        if n > h {
            r = r
                .detail(format!("carton {n} is out (from the cached manifest)"))
                .next(format!("ciginstaller carton {n}"));
            newer = Some(n.to_string());
        }
    }
    Ok(r.data(json!({ "machine_carton": machine, "newer": newer }))
        .into())
}

fn switch(cx: &mut Cx, v: &str) -> Result<Outcome, InstallerError> {
    let mut a = InstallArgs::carton(Some(v.to_string()));
    a.reason = Some(format!("carton {v}"));
    let installed = install::run(cx, &a)?;
    let target = cx
        .manifest(ManifestMode::CacheOnly)
        .ok()
        .and_then(|m| {
            Pin::parse(v)
                .ok()
                .and_then(|p| m.newest_matching(&p, None).map(|c| c.version.to_string()))
        })
        .unwrap_or_else(|| v.to_string());
    let mut r = use_carton::run_locked(
        cx,
        &UseArgs {
            carton: target,
            dry_run: false,
        },
    )?;
    r.detail.insert(0, installed.headline);
    Ok(r.into())
}

fn list(cx: &mut Cx, all: bool) -> Result<Outcome, InstallerError> {
    let installed: Vec<String> = cx.state.cartons.keys().cloned().collect();
    let machine = cx.state.machine_carton.clone();
    let m = cx.manifest(ManifestMode::Daily)?;
    let mut r = Report::done(format!("cartons on the {} channel", m.channel));
    let mut rows = Vec::new();
    for c in m.cartons.values().rev() {
        if c.version.is_prerelease() && !all {
            continue;
        }
        let tools: Vec<String> = c.products.iter().map(|(t, v)| format!("{t} {v}")).collect();
        let mut marks = Vec::new();
        if machine.as_deref() == Some(c.version.to_string().as_str()) {
            marks.push("machine");
        } else if installed.contains(&c.version.to_string()) {
            marks.push("installed");
        }
        if m.yank_of(Yankable::Carton(c.version.clone())).is_some() {
            marks.push("yanked");
        }
        let tag = if marks.is_empty() {
            String::new()
        } else {
            format!("  [{}]", marks.join(", "))
        };
        r = r.detail(format!("{}: {}{tag}", c.version, tools.join(", ")));
        rows.push(json!({ "carton": c.version.to_string(), "tools": tools, "marks": marks }));
    }
    Ok(r.data(json!({ "channel": m.channel, "cartons": rows }))
        .into())
}

fn show(cx: &mut Cx, v: &str) -> Result<Outcome, InstallerError> {
    let verbose = cx.ui.verbose;
    let pin = Pin::parse(v).map_err(|e| {
        InstallerError::new("E1870", e.to_string())
            .with_fix("ciginstaller carton list")
            .with_exit(EXIT_USAGE)
    })?;
    let target = cx.target.to_string();
    let m = cx.manifest(ManifestMode::Daily)?;
    let c = m.newest_matching(&pin, None).ok_or_else(|| {
        InstallerError::new(
            "E1811",
            format!("there is no carton {v} on the {} channel", m.channel),
        )
        .with_fix("ciginstaller carton list")
        .with_exit(EXIT_REFUSED)
    })?;
    let mut r = Report::done(format!("carton {}", c.version));
    if !c.notes.is_empty() {
        r = r.detail(c.notes.clone());
    }
    for (t, ver) in &c.products {
        let mut line = format!("{t} {ver}");
        if verbose {
            let targets: Vec<String> = m
                .builds
                .get(&(t.clone(), ver.clone()))
                .map(|b| b.artifacts.keys().cloned().collect())
                .unwrap_or_default();
            line.push_str(&format!("  targets: {}", targets.join(", ")));
            if m.artifact(t, ver, &cigi_ui_target(&target)).is_none() {
                line.push_str("  (no build for this machine)");
            }
        }
        r = r.detail(line);
    }
    for (name, members) in &c.packs {
        r = r.detail(format!("pack {name}: {}", members.join(", ")));
    }
    if let Some(y) = m.yank_of(Yankable::Carton(c.version.clone())) {
        r = r.detail(format!("yanked: {}", y.reason));
    }
    Ok(r.data(json!({
        "carton": c.version.to_string(),
        "products": c.products.iter().map(|(t, v)| (t.clone(), json!(v.to_string()))).collect::<serde_json::Map<_, _>>(),
        "packs": c.packs,
    })).into())
}

fn cigi_ui_target(s: &str) -> cig_compat::Target {
    cig_compat::Target::parse(s).unwrap_or_else(cig_compat::Target::host)
}
