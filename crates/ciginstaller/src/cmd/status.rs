// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `status`, and what a bare `ciginstaller` shows (#32, #17): the machine's carton and tools,
//! how they were trusted, and what is waiting. At most eight lines. It reads `state.json`, the
//! receipts and the cached manifest, and never opens a socket, so it is as quick offline as on.

use std::collections::BTreeMap;

use cig_compat::{Manifest, Version};
use cigi_home::journal::{list_txns, TxnStatus};
use cigi_home::read_receipt;
use cigi_ui::{InstallerError, Report};
use serde_json::{json, Value};

use super::Outcome;
use crate::cx::Cx;

/// Detail lines after the headline; the next step is the eighth line.
const DETAIL_CAP: usize = 6;

pub fn run(cx: &mut Cx) -> Result<Outcome, InstallerError> {
    let machine = cx.state.machine_carton.clone();
    let channel = cx.channel();
    let tools = effective_tools(cx);
    // the share this machine is in (never the id or the bucket); a stable fact of the machine,
    // so a fleet's own tools can read it with the rest
    let standing = cigi_home::rollout::standing(&cx.cfg.get("rollout"));
    let rollout = json!({
        "percent": standing.percent,
        "in_share": standing.in_share,
        "reason": standing.why.map(|w| w.as_str()),
    });
    let cartons: Vec<&String> = cx.state.cartons.keys().collect();
    let Some(carton) = machine.clone().filter(|_| !tools.is_empty()) else {
        let mut r = Report::done("nothing is installed yet");
        if let Some(l) = interrupted(cx) {
            r = r.detail(l).next("ciginstaller doctor");
        } else {
            r = r.next("ciginstaller install carton");
        }
        return Ok(r
            .data(json!({
                "carton": null,
                "cartons": cartons,
                "channel": channel,
                "rollout": rollout,
                "first_run": crate::flows::install::first_run_view(&cx.state),
                "tools": {},
                "trust": {},
                "waiting": { "update": null, "advisories": [], "yanked": [] },
            }))
            .into());
    };
    let build_of = |t: &str| tools.get(t).and_then(|v| Version::parse(v).ok());

    // how each build was trusted, from its receipt
    let mut trust: BTreeMap<String, u32> = BTreeMap::new();
    for (t, v) in &tools {
        if let Some(rc) = Version::parse(v)
            .ok()
            .and_then(|v| read_receipt(&cx.home, t, &v).ok())
        {
            *trust.entry(rc.trust.level).or_default() += 1;
        }
    }
    let mut lines = Vec::new();
    let mut counts = Vec::new();
    for level in ["pinned", "attested", "checksummed"] {
        if let Some(n) = trust.get(level) {
            counts.push(format!("{level} {n}"));
        }
    }
    let trust_text = if counts.is_empty() {
        String::new()
    } else {
        format!(", trust {}", counts.join(", "))
    };
    lines.push(format!("channel {channel}{trust_text}"));

    // what the cached manifest says; nothing is fetched
    let manifest = cx.peek_manifest();
    let mut update: Option<String> = None;
    let mut advisories: Vec<Value> = Vec::new();
    let mut yanked: Vec<String> = Vec::new();
    let mut waiting_next = false;
    match (&manifest, Version::parse(&carton).ok()) {
        (Some(m), Some(mine)) => {
            if let Some(newest) = newest_over(m, &mine, &channel) {
                let changes: Vec<String> = m
                    .diff(&mine, &newest)
                    .into_iter()
                    .map(|c| {
                        let s =
                            |v: Option<Version>| v.map_or("none".to_string(), |v| v.to_string());
                        format!("{} {} → {}", c.tool, s(c.from), s(c.to))
                    })
                    .collect();
                let what = if changes.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", changes.join(", "))
                };
                lines.push(format!("waiting: carton {newest}{what}"));
                update = Some(newest.to_string());
                waiting_next = true;
            }
            if m.is_carton_yanked(&mine) {
                yanked.push(format!("carton {mine}"));
            }
            for (t, v) in &tools {
                if let Some(b) = Version::parse(v).ok().filter(|b| m.is_product_yanked(t, b)) {
                    yanked.push(format!("{t} {b}"));
                }
            }
            if !yanked.is_empty() {
                lines.push(format!("yanked: {}", yanked.join(", ")));
                waiting_next = true;
            }
            for a in &m.advisories {
                let hit = a.affects.iter().find(|x| {
                    if x.subject == "carton" {
                        x.range.matches_core(&mine)
                    } else {
                        build_of(&x.subject).is_some_and(|b| x.range.matches_core(&b))
                    }
                });
                let Some(hit) = hit else { continue };
                let fixed = a
                    .fixed_in
                    .as_ref()
                    .map(|f| format!(", fixed in {f}"))
                    .unwrap_or_default();
                lines.push(format!(
                    "advisory {} ({}) affects {}{fixed}",
                    a.id,
                    a.severity.as_str(),
                    hit.subject
                ));
                advisories.push(json!({
                    "id": a.id,
                    "severity": a.severity.as_str(),
                    "subject": hit.subject,
                    "fixed_in": a.fixed_in.as_ref().map(|f| f.to_string()),
                }));
                waiting_next = true;
            }
        }
        (None, _) => {
            lines.push("no manifest is cached, so nothing is known to be waiting".to_string())
        }
        _ => {}
    }
    // programs built on a cig that an advisory covers (#30): one line each, read from the cache
    let program_hits = crate::flows::programs::advisory_hits(cx);
    for (f, advs) in &program_hits {
        lines.push(crate::flows::programs::hit_line(cx, f, advs));
        waiting_next = true;
    }

    if let Some(p) = standing.percent.filter(|p| *p < 100) {
        lines.push(format!(
            "rollout {p}%: this machine is {}",
            if standing.in_share {
                "in the share"
            } else {
                "not in the share yet"
            }
        ));
    } else if standing.percent.is_none() {
        lines.push(format!(
            "rollout {}: this machine waits for its share",
            standing.share_text()
        ));
    }

    // how this machine differs from a plain carton
    for s in cx.state.sticks.iter().filter(|s| s.is_rollback()) {
        lines.push(format!("{} is rolled back to {}", s.tool, s.version));
    }
    if cx.state.cartons.get(&carton).is_some_and(|r| r.partial) {
        lines.push(format!("carton {carton} has lost a tool"));
    }
    let broken = interrupted(cx);
    if let Some(l) = &broken {
        lines.push(l.clone());
    }

    let mut r = Report::done(format!(
        "carton {carton}: {}",
        ordered(&tools)
            .iter()
            .map(|(t, v)| format!("{t} {v}"))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    let spill = lines.len().saturating_sub(DETAIL_CAP);
    let shown = if spill > 0 {
        DETAIL_CAP - 1
    } else {
        lines.len()
    };
    for l in lines.iter().take(shown) {
        r = r.detail(l.clone());
    }
    if spill > 0 {
        r = r.detail(format!("and {} more: ciginstaller check", spill + 1));
    }
    if broken.is_some() {
        r = r.next("ciginstaller doctor");
    } else if waiting_next {
        r = r.next("ciginstaller update");
    }
    // `programs` is there only when one has an advisory: a machine without any reads as before
    let mut waiting = json!({ "update": update, "advisories": advisories, "yanked": yanked });
    if !program_hits.is_empty() {
        waiting["programs"] = json!(program_hits
            .iter()
            .map(|(f, advs)| json!({
                "name": f.program.name,
                "path": f.path.display().to_string(),
                "cig": f.program.cig,
                "advisories": advs.iter().map(|a| a.id.clone()).collect::<Vec<_>>(),
            }))
            .collect::<Vec<_>>());
    }
    let trust_json: serde_json::Map<String, Value> =
        trust.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
    Ok(r.data(json!({
        "carton": carton,
        "cartons": cartons,
        "channel": channel,
        "rollout": rollout,
        "first_run": crate::flows::install::first_run_view(&cx.state),
        "tools": tools,
        "trust": trust_json,
        "waiting": waiting,
    }))
    .into())
}

/// The newest carton on the channel that is above the machine's (pre-releases only off stable).
fn newest_over(m: &Manifest, mine: &Version, channel: &str) -> Option<Version> {
    let newest = m.latest(channel != "stable")?.version.clone();
    (newest > *mine).then_some(newest)
}

/// What runs on the machine carton's own terms: its tools, a rolled-back tool's older build, and
/// any other stick for a tool the carton lacks.
pub(crate) fn effective_tools(cx: &Cx) -> BTreeMap<String, String> {
    let s = &cx.state;
    let mut t: BTreeMap<String, String> = s
        .machine_carton
        .as_ref()
        .and_then(|c| s.cartons.get(c))
        .map(|r| r.tools.clone())
        .unwrap_or_default();
    for st in &s.sticks {
        if st.is_rollback() || !t.contains_key(&st.tool) {
            t.insert(st.tool.clone(), st.version.clone());
        }
    }
    t
}

/// The family's order first (cig, cigo, cigcompile, cigar), then any other tool by name.
pub(crate) fn ordered(t: &BTreeMap<String, String>) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = cigi_shim::TOOLS
        .iter()
        .filter_map(|n| t.get(*n).map(|v| (n.to_string(), v.clone())))
        .collect();
    out.extend(
        t.iter()
            .filter(|(n, _)| !cigi_shim::TOOLS.contains(&n.as_str()))
            .map(|(n, v)| (n.clone(), v.clone())),
    );
    out
}

/// A change that was cut short and has not been finished or undone yet.
fn interrupted(cx: &Cx) -> Option<String> {
    list_txns(&cx.home)
        .iter()
        .any(|t| matches!(t.status, TxnStatus::Open | TxnStatus::Unreadable))
        .then(|| "an interrupted change is waiting to be finished".to_string())
}
