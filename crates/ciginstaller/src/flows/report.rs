// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `report` (#11): the version block and the bundle behind a bug report.
//!
//! Half of a multi-tool bug report is "which versions were you running together?". The version
//! block answers it: the installer, the carton, each tool's build, what the project pins, the OS
//! and kernel, and whether the combination is inside the manifest's guarantees. A combination
//! that is not is flagged, with the pair that does not belong together named.
//!
//! The bundle is that block plus what only the installer knows about the operation that went
//! wrong: its journal (redacted), the trial's differences and the metadata versions it saw. For a
//! cig run, cig builds its own bundle (cig alone can read its run records) and it is embedded.
//!
//! Nothing here writes to the machine's state or opens a socket: it reads `state.json`, the
//! receipts, the cached manifest and the transaction folders, and runs cig's bundle builder.

use std::collections::BTreeMap;
use std::path::Path;

use cig_compat::{Manifest, Version};
use cigi_home::journal::{list_txns, TxnStatus, JOURNAL_FORMAT};
use cigi_home::{read_receipt, verify_toolchain, Home};
use cigi_ui::{InstallerError, EXIT_USAGE};
use serde_json::{json, Map, Value};

use super::doctor::{run_args, Ran};
use super::family::installed_version;
use super::tool_exe;
use crate::cmd::status::{effective_tools, ordered};
use crate::crash::anon;
use crate::cx::Cx;
use crate::project::file_label;
use crate::resolve::{self, Mode};

/// The version block's own format, so a product that embeds it can tell a newer one.
pub const BLOCK_FORMAT: u32 = 1;
/// Journal lines a bundle keeps (the newest); the count of the rest is said.
const JOURNAL_LINES: usize = 400;
/// Bytes kept of one script's diff in the trial's differences.
const DIFF_CAP: usize = 8 * 1024;
/// Time cig's bundle builder may take.
const CIG_SECS: u64 = 60;
/// Hex characters of a build's digest the block shows (the build, not a secret).
const DIGEST_SHOWN: usize = 12;
/// The trial record `update` leaves in its transaction folder.
pub const TRIAL_FILE: &str = "trial.json";

fn usage(msg: impl Into<String>) -> InstallerError {
    InstallerError::new("E1870", msg).with_exit(EXIT_USAGE)
}

/// E1858: the report could not be put together.
pub fn failed(msg: impl Into<String>) -> InstallerError {
    InstallerError::new("E1858", msg).with_fix("ciginstaller report --block")
}

// ------------------------------------------------------------------------- the trial record

/// Called by `update` once the trial has run: the trial's result goes into the transaction's own
/// folder, so a report on that update (rolled back, declined or done) can name what planned
/// differently. Best effort: a report is a convenience, never a reason to fail an update.
pub fn keep_trial(txn_dir: &Path, trial: &Value) {
    let body = json!({ "format": 1, "at": cigi_home::now_rfc3339(), "trial": trial });
    let _ = std::fs::write(txn_dir.join(TRIAL_FILE), body.to_string());
}

// ------------------------------------------------------------------------- the version block

/// One thing that does not fit: `kind` is `mixed`, `requires`, `pin`, `yanked` or `state`.
struct Mismatch {
    kind: &'static str,
    text: String,
}

fn kernel() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

fn platform() -> Value {
    json!({ "os": std::env::consts::OS, "arch": std::env::consts::ARCH, "kernel": kernel() })
}

/// The version block when `state.json` cannot be read: what is known without it, and the
/// reason, so a report on a broken install still says which installer it came from.
pub fn block_without_state(why: &InstallerError) -> Value {
    json!({
        "format": BLOCK_FORMAT,
        "installer": { "version": env!("CARGO_PKG_VERSION"), "channel": null, "healthy": null },
        "platform": platform(),
        "carton": { "machine": null, "channel": null, "previous": null, "partial": false },
        "tools": [],
        "project": null,
        "guarantee": { "inside": null, "carton": null, "checked_against": "nothing: state.json cannot be read" },
        "mismatches": [{ "kind": "state", "text": format!("{} {}", why.code, why.message) }],
        "metadata": null,
    })
}

/// The block, from what this machine holds. Reads only; a manifest that is not cached is not
/// fetched (the guarantee is then checked against the installed cartons alone, and says so).
pub fn version_block(cx: &mut Cx) -> Value {
    let tools = effective_tools(cx);
    let named = ordered(&tools);
    let machine = cx.state.machine_carton.clone();
    let manifest = cx.peek_manifest();
    let mut mismatches: Vec<Mismatch> = Vec::new();

    // what the project pins, and what it therefore runs
    let mut running: BTreeMap<String, String> = tools.clone();
    let mut runs: Vec<Value> = Vec::new();
    let project = match cx.project() {
        Ok(Some(p)) => {
            let carton_pin = p
                .carton_pin
                .as_ref()
                .map(|(pin, f)| json!({ "pin": pin.to_string(), "file": file_label(f) }));
            let legacy: Vec<Value> = p
                .legacy
                .iter()
                .map(|(t, pin, f)| json!({ "tool": t, "pin": pin.to_string(), "file": file_label(f) }))
                .collect();
            if carton_pin.is_some() || !legacy.is_empty() {
                for (tool, _) in &named {
                    match resolve::resolve(cx, tool, Mode::Which) {
                        Ok(r) => {
                            running.insert(tool.clone(), r.build.to_string());
                            runs.push(json!({
                                "tool": tool, "build": r.build.to_string(),
                                "carton": r.carton.as_ref().map(ToString::to_string),
                                "why": r.why_text(),
                            }));
                        }
                        Err(e) => mismatches.push(Mismatch {
                            kind: "pin",
                            text: format!(
                                "the project's pins cannot be met for {tool}: {}",
                                e.message
                            ),
                        }),
                    }
                }
            }
            json!({ "carton_pin": carton_pin, "legacy_pins": legacy, "runs": runs })
        }
        Ok(None) => Value::Null,
        Err(e) => {
            mismatches.push(Mismatch {
                kind: "pin",
                text: format!("the project's pin file cannot be used: {}", e.message),
            });
            Value::Null
        }
    };

    // every tool's build: its version, the digest of what was installed, how it was trusted
    let mut tool_rows = Vec::new();
    for (tool, ver) in &named {
        let parsed = Version::parse(ver).ok();
        let receipt = parsed
            .as_ref()
            .and_then(|v| read_receipt(&cx.home, tool, v).ok());
        let carton_has = machine
            .as_ref()
            .and_then(|c| cx.state.cartons.get(c))
            .and_then(|r| r.tools.get(tool))
            .cloned();
        let stick = cx.state.sticks.iter().find(|s| &s.tool == tool);
        let state = match stick {
            Some(s) if s.is_rollback() => "rolled-back",
            Some(_) => "stick",
            None => "carton",
        };
        tool_rows.push(json!({
            "tool": tool,
            "version": ver,
            "build": receipt.as_ref().map(|r| format!("sha256:{}", &r.artifact.sha256[..r.artifact.sha256.len().min(DIGEST_SHOWN)])),
            "trust": receipt.as_ref().map(|r| r.trust.level.clone()),
            "from_carton": receipt.as_ref().and_then(|r| r.from_carton.clone()),
            "state": state,
            "carton_has": carton_has,
            "runs_in_project": running.get(tool).filter(|b| *b != ver),
        }));
        if let (Some(m), Some(v)) = (&manifest, &parsed) {
            if m.is_product_yanked(tool, v) {
                mismatches.push(Mismatch {
                    kind: "yanked",
                    text: format!("{tool} {ver} is yanked in the manifest"),
                });
            }
        }
    }

    // the combination: one carton that holds every build that runs, or a flagged mix
    let mut groups: Vec<(String, BTreeMap<String, String>)> = cx
        .state
        .cartons
        .iter()
        .map(|(c, r)| (c.clone(), r.tools.clone()))
        .collect();
    if let Some(m) = &manifest {
        for (cv, c) in &m.cartons {
            let name = cv.to_string();
            if !groups.iter().any(|(g, _)| *g == name) {
                groups.push((
                    name,
                    c.products
                        .iter()
                        .map(|(t, v)| (t.clone(), v.to_string()))
                        .collect(),
                ));
            }
        }
    }
    let names: Vec<String> = ordered(&running).into_iter().map(|(t, _)| t).collect();
    let mut mixed = 0usize;
    for i in 0..names.len() {
        for j in (i + 1)..names.len() {
            let (a, b) = (&names[i], &names[j]);
            let (va, vb) = (&running[a], &running[b]);
            let together = groups
                .iter()
                .any(|(_, g)| g.get(a) == Some(va) && g.get(b) == Some(vb));
            if !together {
                mixed += 1;
                if mixed <= 5 {
                    mismatches.push(Mismatch {
                        kind: "mixed",
                        text: format!("{b} {vb} with {a} {va}: not from the same carton"),
                    });
                }
            }
        }
    }
    if mixed > 5 {
        mismatches.push(Mismatch {
            kind: "mixed",
            text: format!(
                "and {} more pairs that are not from the same carton",
                mixed - 5
            ),
        });
    }
    if let Some(m) = &manifest {
        requirements(m, &running, &mut mismatches);
    }
    let broken = mismatches
        .iter()
        .any(|m| matches!(m.kind, "mixed" | "requires"));
    let whole = groups
        .iter()
        .filter(|(_, g)| running.iter().all(|(t, v)| g.get(t) == Some(v)))
        .map(|(c, _)| c.clone())
        .next();
    let inside = if running.is_empty() {
        None
    } else {
        Some(!broken && whole.is_some())
    };

    let rows: Vec<Value> = mismatches
        .iter()
        .map(|m| json!({ "kind": m.kind, "text": m.text }))
        .collect();
    json!({
        "format": BLOCK_FORMAT,
        "installer": {
            "version": env!("CARGO_PKG_VERSION"),
            "channel": cx.channel(),
            "healthy": cx.state.installer.healthy,
        },
        "platform": platform(),
        "carton": {
            "machine": machine,
            "channel": cx.state.channel,
            "previous": cx.state.previous.machine_carton,
            "partial": machine.as_ref().and_then(|c| cx.state.cartons.get(c)).is_some_and(|r| r.partial),
        },
        "tools": tool_rows,
        "project": project,
        "guarantee": {
            "inside": inside,
            "carton": if inside == Some(true) { whole } else { None },
            "checked_against": match &manifest {
                Some(m) => format!("manifest sequence {} and the installed cartons", m.sequence),
                None => "the installed cartons only: no manifest is cached".to_string(),
            },
        },
        "mismatches": rows,
        "metadata": metadata(cx, manifest.as_ref(), &running),
    })
}

/// A build that needs another tool in a range the build that runs is outside of.
fn requirements(m: &Manifest, running: &BTreeMap<String, String>, out: &mut Vec<Mismatch>) {
    for (tool, ver) in running {
        let Some(v) = Version::parse(ver).ok() else {
            continue;
        };
        let Some(req) = m.product(tool, &v) else {
            continue;
        };
        for (dep, range) in &req.requires {
            let Some(dv) = running.get(dep).and_then(|d| Version::parse(d).ok()) else {
                continue;
            };
            if !range.matches(&dv) {
                out.push(Mismatch {
                    kind: "requires",
                    text: format!(
                        "{tool} {ver} needs {dep} {}, and {dep} {dv} is what runs",
                        range.text
                    ),
                });
            }
        }
        for (dep, range) in &req.recommends {
            let Some(dv) = running.get(dep).and_then(|d| Version::parse(d).ok()) else {
                continue;
            };
            if !range.matches(&dv) {
                out.push(Mismatch {
                    kind: "recommends",
                    text: format!(
                        "{tool} {ver} recommends {dep} {}, and {dep} {dv} is what runs",
                        range.text
                    ),
                });
            }
        }
    }
}

/// The metadata versions this machine holds: the manifest it last admitted, the formats it
/// reads, and the manifest sequence each build was trusted under.
fn metadata(cx: &Cx, cached: Option<&Manifest>, running: &BTreeMap<String, String>) -> Value {
    let mut receipts = Map::new();
    let mut embeds = Map::new();
    for (tool, ver) in running {
        let Some(v) = Version::parse(ver).ok() else {
            continue;
        };
        if let Ok(r) = read_receipt(&cx.home, tool, &v) {
            receipts.insert(tool.clone(), json!(r.trust.manifest_sequence));
        }
        if let Some(p) = cached.and_then(|m| m.product(tool, &v)) {
            embeds.insert(
                tool.clone(),
                json!({ "journal_format": p.journal_format(), "cig_runtime": p.cig_runtime().map(|v| v.to_string()) }),
            );
        }
    }
    json!({
        "manifest": cx.state.manifest.as_ref().map(|m| json!({
            "sequence": m.sequence, "channel": m.channel, "trust": m.trust_level,
            "fetched_at": m.fetched_at, "expires": m.expires,
        })),
        "cached_manifest": cached.map(|m| json!({
            "format": m.format, "sequence": m.sequence, "generated": m.generated,
        })),
        "state_format": cx.state.format,
        "journal_format": JOURNAL_FORMAT,
        "receipt_sequences": receipts,
        "products": embeds,
    })
}

/// The block as lines for a person: the headline says whether anything is flagged.
pub fn block_lines(b: &Value) -> Vec<String> {
    let s = |v: &Value| v.as_str().unwrap_or("?").to_string();
    let mut v = Vec::new();
    let c = &b["carton"];
    v.push(format!(
        "installer {}, carton {} on {}, {} {}{}",
        s(&b["installer"]["version"]),
        c["machine"].as_str().unwrap_or("none"),
        c["channel"].as_str().unwrap_or("?"),
        s(&b["platform"]["os"]),
        s(&b["platform"]["arch"]),
        b["platform"]["kernel"]
            .as_str()
            .map(|k| format!(", kernel {k}"))
            .unwrap_or_default(),
    ));
    for t in b["tools"].as_array().into_iter().flatten() {
        let mut line = format!("{} {}", s(&t["tool"]), s(&t["version"]));
        let mut notes = Vec::new();
        if let Some(d) = t["build"].as_str() {
            notes.push(d.to_string());
        }
        if let Some(l) = t["trust"].as_str() {
            notes.push(l.to_string());
        }
        if t["state"] != "carton" {
            notes.push(s(&t["state"]));
        }
        if let Some(r) = t["runs_in_project"].as_str() {
            notes.push(format!("the project runs {r}"));
        }
        if !notes.is_empty() {
            line.push_str(&format!(" ({})", notes.join(", ")));
        }
        v.push(line);
    }
    if let Some(p) = b["project"].as_object() {
        let mut pins = Vec::new();
        if let Some(c) = p["carton_pin"].as_object() {
            pins.push(format!("carton {} ({})", s(&c["pin"]), s(&c["file"])));
        }
        for l in p["legacy_pins"].as_array().into_iter().flatten() {
            pins.push(format!(
                "{} {} ({})",
                s(&l["tool"]),
                s(&l["pin"]),
                s(&l["file"])
            ));
        }
        v.push(if pins.is_empty() {
            "the project pins nothing".to_string()
        } else {
            format!("the project pins {}", pins.join(", "))
        });
    }
    v.push(match b["guarantee"]["inside"].as_bool() {
        Some(true) => format!(
            "inside the manifest's guarantees: carton {}",
            b["guarantee"]["carton"].as_str().unwrap_or("?")
        ),
        Some(false) => "NOT inside the manifest's guarantees".to_string(),
        None => "the manifest's guarantees were not checked".to_string(),
    });
    for m in b["mismatches"].as_array().into_iter().flatten() {
        v.push(format!("flagged: {}", s(&m["text"])));
    }
    v
}

pub fn flagged(b: &Value) -> usize {
    b["mismatches"].as_array().map_or(0, Vec::len)
}

// ------------------------------------------------------------------------- the bundle

/// `run` as an installer operation: a history id or transaction id (or a prefix of either);
/// with no run named, the newest operation. `Ok(None)` when nothing matches.
pub fn find_operation(cx: &Cx, run: Option<&str>) -> Result<Option<String>, InstallerError> {
    let txns = list_txns(&cx.home);
    let mut ids: Vec<String> = txns.iter().map(|t| t.id.clone()).collect();
    for h in &cx.state.history {
        if !ids.contains(&h.txn) && !h.txn.is_empty() {
            ids.push(h.txn.clone());
        }
    }
    ids.sort();
    let Some(want) = run else {
        return Ok(ids.pop());
    };
    let by_history: Vec<&str> = cx
        .state
        .history
        .iter()
        .filter(|h| h.id == want)
        .map(|h| h.txn.as_str())
        .collect();
    if let [one] = by_history.as_slice() {
        return Ok(Some(one.to_string()));
    }
    let mut hit: Vec<String> = ids
        .iter()
        .filter(|i| i.starts_with(want))
        .cloned()
        .collect();
    for h in cx.state.history.iter().filter(|h| h.id.starts_with(want)) {
        if !hit.contains(&h.txn) {
            hit.push(h.txn.clone());
        }
    }
    match hit.len() {
        0 => Ok(None),
        1 => Ok(hit.pop()),
        n => Err(usage(format!(
            "`{want}` matches {n} operations; give more of the id"
        ))
        .with_fix("ciginstaller history")),
    }
}

fn status_word(s: TxnStatus) -> &'static str {
    match s {
        TxnStatus::Committed => "committed",
        TxnStatus::RolledBack => "rolled-back",
        TxnStatus::Open => "interrupted",
        TxnStatus::Unreadable => "unreadable",
    }
}

fn cap(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}... (cut)", &s[..end])
}

/// The installer's record of one operation: its history line, its journal (the newest lines,
/// each one JSON object, the process id removed), the trial's differences.
pub fn operation(home: &Home, history: &[cigi_home::state::HistoryEntry], txn: &str) -> Value {
    let safe = !txn.is_empty() && txn.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    let dir = home.txn_root().join(txn);
    let info = list_txns(home).into_iter().find(|t| t.id == txn);
    let mut lines: Vec<Value> = Vec::new();
    if safe {
        if let Ok(text) = std::fs::read_to_string(dir.join("journal.jsonl")) {
            lines = text
                .lines()
                .filter_map(|l| serde_json::from_str::<Value>(l).ok())
                .map(|mut v| {
                    if let Some(o) = v.as_object_mut() {
                        o.remove("pid");
                    }
                    v
                })
                .collect();
        }
    }
    let total = lines.len();
    let drop = total.saturating_sub(JOURNAL_LINES);
    let lines: Vec<Value> = lines.into_iter().skip(drop).collect();
    let trial = safe
        .then(|| std::fs::read_to_string(dir.join(TRIAL_FILE)).ok())
        .flatten()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .map(|mut t| {
            // the diffs are the biggest thing a trial holds: each is cut, the count stays
            if let Some(ds) = t["trial"]["different"].as_array_mut() {
                for d in ds {
                    if let Some(x) = d["diff"].as_str().map(|s| cap(s, DIFF_CAP)) {
                        d["diff"] = Value::String(x);
                    }
                }
            }
            t
        });
    let entry = history.iter().find(|h| h.txn == txn).map(|h| {
        json!({ "id": h.id, "at": h.at, "what": h.what, "from": h.from, "to": h.to, "ok": h.ok })
    });
    json!({
        "id": txn,
        "label": info.as_ref().map(|i| i.label.clone()),
        "status": info.as_ref().map_or("not on disk", |i| status_word(i.status)),
        "history": entry,
        "journal_lines": total,
        "journal_cut": drop,
        "journal": lines,
        "trial": trial.as_ref().map(|t| t["trial"].clone()),
        "trial_at": trial.as_ref().map(|t| t["at"].clone()),
    })
}

/// A cig run id is a word: nothing that could be read as a flag, nothing with a path in it.
fn plain_run_id(run: &str) -> Result<(), InstallerError> {
    let ok = !run.is_empty()
        && run.len() <= 80
        && !run.starts_with('-')
        && run
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if ok {
        Ok(())
    } else {
        Err(usage(format!("`{}` is not a run id", cap(run, 40))).with_fix("ciginstaller history"))
    }
}

fn said(r: &Ran) -> String {
    let t = format!("{}{}", r.stderr, r.stdout);
    let lines: Vec<&str> = t.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let from = lines.len().saturating_sub(2);
    let j = lines[from..].join(" / ");
    if j.is_empty() {
        "it said nothing".to_string()
    } else {
        cap(&j, 300)
    }
}

fn bundle_of(r: &Ran) -> Option<Value> {
    (r.code == Some(0))
        .then(|| serde_json::from_str::<Value>(r.stdout.trim()).ok())
        .flatten()
        .filter(Value::is_object)
}

/// cig does not have the subcommand: clap's words, an exit 2 and no bundle.
fn unknown_subcommand(r: &Ran) -> bool {
    r.code == Some(2)
        && (r.stderr.contains("unrecognized subcommand")
            || r.stderr.contains("unexpected argument"))
}

/// cig's own bundle for one of its runs. The contract names `cig report-bundle <run> --json`;
/// the cig on main (1.20.x) has only `cig --json report <run>`, which prints the same bundle on
/// stdout, so that is the fallback, used only when cig says it does not know the first.
pub fn cig_bundle(cx: &Cx, run: &str) -> Result<Value, InstallerError> {
    plain_run_id(run)?;
    let Some(ver) = installed_version(&cx.state, "cig") else {
        return Err(usage(format!(
            "no operation of this machine matches `{run}`, and cig is not installed to look for a run of that name"
        ))
        .with_fix("ciginstaller history"));
    };
    let exe = tool_exe(&cx.home, "cig", &ver).ok_or_else(|| {
        failed(format!(
            "cig {ver} has no readable receipt, so it cannot build the bundle"
        ))
        .with_fix("ciginstaller repair")
    })?;
    if !verify_toolchain(&cx.home, "cig", &ver).is_empty() {
        return Err(failed(format!(
            "cig {ver} differs from its receipt, so it is not run to build a bundle"
        ))
        .with_fix("ciginstaller repair"));
    }
    let (bin, root) = (cx.home.bin(), cx.home.root());
    let go = |args: &[&str]| {
        run_args(&exe, args, &root, CIG_SECS, Some(&bin))
            .map_err(|why| failed(format!("cig could not be run to build the bundle: {why}")))
    };
    let first = go(&["report-bundle", run, "--json"])?;
    if let Some(b) = bundle_of(&first) {
        return Ok(b);
    }
    let last = if unknown_subcommand(&first) {
        let second = go(&["--json", "report", run])?;
        if let Some(b) = bundle_of(&second) {
            return Ok(b);
        }
        second
    } else {
        first
    };
    if last.code == Some(3) {
        return Err(
            usage(format!("cig has no run matching `{run}`: {}", said(&last))).with_fix("cig runs"),
        );
    }
    Err(failed(format!(
        "cig {ver} did not give a bundle for `{run}` ({}): {}",
        last.code
            .map_or("ended by a signal".to_string(), |c| format!("exit {c}")),
        said(&last)
    )))
}

/// The whole bundle: the block, the installer's record of the operation, cig's bundle for a cig
/// run. Every string is anonymised once, here, so what `--out` writes is what `--send` carries.
pub fn bundle(
    block: Value,
    operation: Option<Value>,
    cig: Option<Value>,
    run_id: Option<&str>,
    root: &Path,
) -> Value {
    let mut b = json!({
        "kind": "ciginstaller-report",
        "format": 1,
        "installer": env!("CARGO_PKG_VERSION"),
        "generated": cigi_home::now_rfc3339(),
        "run": run_id,
        "version_block": block,
        "operation": operation,
        "cig_bundle": cig,
        "note": "Anonymised like a crash report: the person's paths are shapes, user and host names and secret-shaped values are removed, file contents are never included. Nothing in it names the person or the machine. Read it before you send it.",
    });
    anon::anonymise_value(&mut b, Some(root));
    b
}

/// One line for the ashtray: what the report is about.
pub fn what(b: &Value) -> String {
    if let Some(l) = b["operation"]["label"].as_str() {
        return format!("{l} ({})", b["operation"]["status"].as_str().unwrap_or("?"));
    }
    if b["cig_bundle"].is_object() {
        let code = b["cig_bundle"]["diagnostic"]["code"]
            .as_str()
            .unwrap_or("none");
        return format!("cig run bundle, {code}");
    }
    "version block".to_string()
}
