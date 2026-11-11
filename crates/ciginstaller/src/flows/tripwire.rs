// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! The installer's tripwires (#28 part 1): the promises `update` and `rollback` make, checked
//! once the work is done and read back from disk, not from what the flow still holds in memory.
//!
//! - After an `update`: every shim resolves to the carton's build (the index names the toolchain
//!   file its receipt says to run, and that file is there), every name in `bin/` is there, and the
//!   index says what `state.json` says.
//! - After a `rollback`: the toolchains it went back to match their receipts byte for byte, and
//!   the same index checks hold.
//!
//! A tripped wire never undoes anything and never sends anything. The command says so
//! (`E1849`), and a report of kind `tripwire`, origin `ciginstaller`, goes to the ashtray where
//! `ciginstaller crash list` shows it beside every other product's. A tripwire on rollback is
//! marked high: it means the undo may be wrong.

use std::path::{Path, PathBuf};

use cig_compat::Version;
use cigi_home::journal::fault_planted;
use cigi_home::toolchain::{read_receipt, verify_toolchain, Damage};
use cigi_home::{shim_path, Home, ShimIndex, State};
use cigi_ui::Report;
use serde_json::{json, Value};

use super::switch::shim_names;
use super::tool_exe;
use crate::crash::{self, anon, Kind};

/// The code a tripped wire carries.
pub const CODE: &str = "E1849";

/// How many lines of one kind a report carries before it says "and N more".
const SHOWN: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wire {
    AfterUpdate,
    AfterRollback,
}

impl Wire {
    pub fn word(self) -> &'static str {
        match self {
            Wire::AfterUpdate => "update",
            Wire::AfterRollback => "rollback",
        }
    }
    fn severity(self) -> &'static str {
        match self {
            Wire::AfterUpdate => "medium",
            Wire::AfterRollback => "high",
        }
    }
}

/// Check the wire's promises. Empty when they all hold.
pub fn check(home: &Home, wire: Wire) -> Vec<String> {
    let state = match State::load(home) {
        Ok(s) => s,
        Err(e) => return vec![format!("state.json cannot be read back: {e}")],
    };
    plant(home, &state, wire);
    let mut out = shims_and_index(home, &state);
    if wire == Wire::AfterRollback {
        out.extend(toolchains(home, &state));
    }
    out
}

/// Check, and when a wire trips: say so in `report`, and save a report in the ashtray. The
/// report is returned unchanged when everything holds.
pub fn finish(home: &Home, wire: Wire, mut report: Report) -> Report {
    let problems = check(home, wire);
    if problems.is_empty() {
        return report;
    }
    let root = home.root();
    let headline = format!(
        "after the {}, {} promise(s) did not hold",
        wire.word(),
        problems.len()
    );
    report = report.detail(format!(
        "warning {CODE} tripwire: {headline}; the {} itself is done and nothing was undone",
        wire.word()
    ));
    for p in problems.iter().take(SHOWN) {
        report = report.detail(format!("  {p}"));
    }
    if problems.len() > SHOWN {
        report = report.detail(format!("  and {} more", problems.len() - SHOWN));
    }
    let saved = save(&root, wire, &problems);
    let mut id = None;
    match &saved {
        Ok(i) => {
            report = report.detail(format!("a report is saved ({i}); nothing was sent"));
            report = report.next(format!("ciginstaller crash show {i}"));
            id = Some(i.clone());
        }
        Err(e) => {
            report = report.detail(format!("the report could not be saved ({e})"));
            report = report.next("ciginstaller doctor");
        }
    }
    let mut data = match std::mem::take(&mut report.data) {
        Value::Object(m) => m,
        _ => serde_json::Map::new(),
    };
    data.insert(
        "tripwires".into(),
        json!([{ "code": CODE, "wire": wire.word(), "severity": wire.severity(),
                 "problems": problems, "report": id }]),
    );
    report.data = Value::Object(data);
    report
}

/// The report: kind tripwire, origin ciginstaller, written to the ashtray and left there.
fn save(root: &Path, wire: Wire, problems: &[String]) -> Result<String, String> {
    // the message is the same for the same wire, so the same fault is one fingerprint
    let message = format!("tripwire after {}: a promise did not hold", wire.word());
    let mut r = crash::new_report(
        crash::ORIGIN,
        env!("CARGO_PKG_VERSION"),
        Kind::Tripwire,
        &format!("tripwire:after-{}", wire.word()),
        &message,
        Vec::new(),
        Some(root),
    );
    r.title = Some(message);
    r.details = Some(json!({
        "wire": wire.word(),
        "severity": wire.severity(),
        "problems": problems
            .iter()
            .map(|p| anon::anonymise_text_in(p, Some(root)))
            .collect::<Vec<_>>(),
    }));
    crash::write_new(root, &mut r).map_err(|e| e.to_string())?;
    Ok(r.id)
}

/// A path as the lines show it: inside the home, from the home.
fn rel(home: &Home, p: &Path) -> String {
    p.strip_prefix(home.root())
        .unwrap_or(p)
        .display()
        .to_string()
        .replace('\\', "/")
}

/// The index on disk against `state.json`, every shim against the carton's build.
fn shims_and_index(home: &Home, state: &State) -> Vec<String> {
    let mut out = Vec::new();
    let text = match std::fs::read_to_string(home.shim_index_path()) {
        Ok(t) => t,
        Err(e) => return vec![format!("shim-index cannot be read back: {e}")],
    };
    // either form: an entry of the older absolute form that lies inside this home is read as
    // the relative one; one outside it stays absolute and shows up below as a difference
    let Some(idx) = ShimIndex::parse(&text).map(|i| i.normalized(&home.root())) else {
        return vec!["shim-index is not an index this build reads".to_string()];
    };
    let want = ShimIndex::from_state(home, state);
    if idx.machine != want.machine {
        out.push(format!(
            "shim-index says the machine carton is {}, state.json says {}",
            idx.machine.as_deref().unwrap_or("none"),
            want.machine.as_deref().unwrap_or("none")
        ));
    }
    let mut diffs = Vec::new();
    for (c, tools) in &want.cartons {
        for (t, p) in tools {
            match idx.cartons.get(c).and_then(|x| x.get(t)) {
                Some(got) if got == p => {
                    if idx.identities.get(p) != want.identities.get(p) {
                        diffs.push(format!(
                            "shim-index's identity record for {t} of carton {c} differs from the receipt"
                        ));
                    }
                }
                Some(got) => diffs.push(format!(
                    "shim-index runs {t} of carton {c} from {}, the receipt says {}",
                    rel(home, got),
                    rel(home, p)
                )),
                None => diffs.push(format!("shim-index has no {t} for carton {c}")),
            }
        }
    }
    for c in idx
        .cartons
        .keys()
        .filter(|c| !want.cartons.contains_key(*c))
    {
        diffs.push(format!(
            "shim-index names carton {c}, which state.json does not"
        ));
    }
    for (t, p) in &want.sticks {
        if idx.sticks.get(t) != Some(p) {
            diffs.push(format!(
                "shim-index's stick for {t} differs from state.json"
            ));
        }
    }
    out.extend(diffs);

    // what each shim of the machine carton resolves to, the way the shim reads the index
    if let Some(rec) = state
        .machine_carton
        .as_ref()
        .and_then(|m| state.cartons.get(m))
    {
        for (tool, ver) in &rec.tools {
            let Ok(v) = Version::parse(ver) else { continue };
            // a tool rolled back on its own runs the stick's build: the index says so
            let build = state
                .sticks
                .iter()
                .find(|s| s.is_rollback() && &s.tool == tool)
                .map_or((ver.clone(), v.clone()), |s| {
                    (
                        s.version.clone(),
                        Version::parse(&s.version).unwrap_or(v.clone()),
                    )
                });
            let Some(exe) = tool_exe(home, tool, &build.1) else {
                continue; // no receipt: the toolchain check says so
            };
            // the index holds paths from the home; the build's path here is absolute
            let from_home = |p: PathBuf| {
                if p.is_absolute() {
                    p
                } else {
                    home.root().join(p)
                }
            };
            match idx.pick(None, tool).map(|(c, p)| (c, from_home(p))) {
                Some((_, got)) if got == exe && exe.is_file() => {}
                Some((_, got)) => out.push(format!(
                    "the {tool} shim resolves to {}, the carton's build is {tool} {} ({})",
                    rel(home, &got),
                    build.0,
                    rel(home, &exe)
                )),
                None => out.push(format!(
                    "the {tool} shim resolves to nothing, the carton's build is {tool} {}",
                    build.0
                )),
            }
        }
    }
    for name in shim_names(home, state) {
        let p = shim_path(home, &name);
        if std::fs::symlink_metadata(&p).is_err() {
            out.push(format!("bin/{name} is missing"));
        }
    }
    out
}

/// The builds the machine runs after a rollback, each against its receipt.
fn toolchains(home: &Home, state: &State) -> Vec<String> {
    let mut out = Vec::new();
    for (t, v) in builds(state) {
        let Ok(ver) = Version::parse(&v) else {
            continue;
        };
        if !cigi_home::identity_drift(home, &t, &ver).is_empty() {
            out.push(format!(
                "{t} {v}: the file is not the one its receipt recorded (E1903: the home was moved or restored)"
            ));
        }
        for d in verify_toolchain(home, &t, &ver) {
            out.push(match d {
                Damage::NoReceipt => format!("{t} {v} has no readable receipt"),
                Damage::Missing { path } => format!("{t} {v}: {path} is missing"),
                Damage::Changed { path, .. } => format!("{t} {v}: {path} differs from its receipt"),
                Damage::Mode { path } => format!("{t} {v}: {path} has the wrong mode"),
                Damage::Extra { path } => format!("{t} {v}: {path} is not in its receipt"),
            });
        }
    }
    out
}

/// The tool builds the machine runs: sticks first (a rolled-back tool), then the machine carton.
fn builds(state: &State) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = state
        .sticks
        .iter()
        .map(|s| (s.tool.clone(), s.version.clone()))
        .collect();
    if let Some(rec) = state
        .machine_carton
        .as_ref()
        .and_then(|m| state.cartons.get(m))
    {
        for (t, ver) in &rec.tools {
            if !v.iter().any(|(x, _)| x == t) {
                v.push((t.clone(), ver.clone()));
            }
        }
    }
    v
}

/// Test builds only (`CIGI_FAULT=tripwire-index:1`, `tripwire-toolchain:1`): tamper with what the
/// wire is about to read, so a test sees it trip. Does nothing in any other build.
fn plant(home: &Home, state: &State, wire: Wire) {
    if fault_planted("tripwire-index") {
        if let (Ok(text), Some(m)) = (
            std::fs::read_to_string(home.shim_index_path()),
            state.machine_carton.as_ref(),
        ) {
            let prefix = format!("carton\t{m}\t");
            let mut done = false;
            let edited: Vec<String> = text
                .lines()
                .map(|l| {
                    if !done && l.starts_with(&prefix) {
                        done = true;
                        format!("{l}.stale")
                    } else {
                        l.to_string()
                    }
                })
                .collect();
            let _ = std::fs::write(home.shim_index_path(), edited.join("\n") + "\n");
        }
    }
    if wire == Wire::AfterRollback && fault_planted("tripwire-toolchain") {
        if let Some((t, v)) = builds(state).into_iter().next() {
            if let Ok(ver) = Version::parse(&v) {
                if let Ok(r) = read_receipt(home, &t, &ver) {
                    if let Some(f) = r.files.first() {
                        let p = home.toolchain(&t, &ver).join(&f.path);
                        if let Ok(mut bytes) = std::fs::read(&p) {
                            bytes.push(b'!');
                            // the file may be a link of a store object: break the link first
                            let _ = std::fs::remove_file(&p);
                            let _ = std::fs::write(&p, bytes);
                        }
                    }
                }
            }
        }
    }
}
