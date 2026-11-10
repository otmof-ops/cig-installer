// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! The issue a report becomes (#28 section 5): a body a person can read, labels the pipeline
//! sets, and at the end of the body a fenced, versioned machine block, `cig-report v1`, that the
//! hive (or any tool) reads without scraping prose.
//!
//! Everything here is rendered from `route::travel`'s object (a `Value`), never from the saved
//! report, so nothing outside the allowlist can reach a body, a block, a label or a search.

use serde_json::{json, Map, Value};

use super::blame::Blame;
use super::truncate;

/// The fence's info string, version included.
pub const FENCE: &str = "cig-report v1";
/// The block's own version, repeated inside it.
pub const BLOCK_VERSION: u64 = 1;
/// GitHub refuses a body over 65,536 characters; stay well under.
const BODY_MAX: usize = 60_000;

pub struct Rendered {
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or("")
}

/// A text that cannot close the fence it sits in.
fn safe(text: &str) -> String {
    text.replace("```", "'''")
}

pub fn title(view: &Value) -> String {
    let t = match view["title"].as_str().filter(|t| !t.is_empty()) {
        Some(t) => t.to_string(),
        None => format!("{}: {}", s(view, "origin"), s(view, "panic_message")),
    };
    let one_line: String = t.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate(&one_line, 120)
}

fn rollback_tripwire(view: &Value) -> bool {
    s(view, "kind") == "tripwire"
        && ["panic_location", "panic_message", "title"]
            .iter()
            .any(|k| s(view, k).to_ascii_lowercase().contains("rollback"))
}

fn regression(view: &Value) -> bool {
    view["details"]["regression"].as_bool().unwrap_or(false)
        || view["details"]["first_bad_carton"].is_string()
}

pub fn severity(view: &Value) -> &'static str {
    if rollback_tripwire(view) {
        "high"
    } else if matches!(s(view, "kind"), "report" | "update") {
        "low"
    } else {
        "medium"
    }
}

/// kind, area, regression, severity. A tripwire on rollback is high: undo may be wrong.
pub fn labels(view: &Value, blame: &Blame) -> Vec<String> {
    let mut l = vec![
        format!("kind:{}", s(view, "kind")),
        format!("area:{}", blame.product),
        format!("severity:{}", severity(view)),
    ];
    if regression(view) {
        l.push("regression".into());
    }
    l
}

/// The block, as a value.
pub fn machine_block(view: &Value, blame: &Blame) -> Value {
    let details = &view["details"];
    let matrix = details["version_matrix"]
        .as_object()
        .cloned()
        .unwrap_or_else(|| {
            let mut m = Map::new();
            m.insert(s(view, "origin").to_string(), json!(s(view, "version")));
            m
        });
    json!({
        "schema": "cig-report",
        "version": BLOCK_VERSION,
        "id": s(view, "id"),
        "fingerprint": s(view, "fingerprint"),
        "kind": s(view, "kind"),
        "origin": s(view, "origin"),
        "origin_version": s(view, "version"),
        "os": s(view, "os"),
        "arch": s(view, "arch"),
        "blame": { "product": blame.product, "repo": blame.repo, "via": blame.via.word() },
        "matrix": matrix,
        "carton": details["carton"].as_str(),
        "repro": view.get("repro").filter(|r| r.is_object()),
        "first_bad_carton": details["first_bad_carton"].as_str(),
        "regression": regression(view),
        "labels": labels(view, blame),
    })
}

/// The block as the fenced text. Backticks inside it are written ```: still the same JSON,
/// and nothing in it can close the fence.
pub fn fenced(block: &Value) -> String {
    let json = serde_json::to_string_pretty(block)
        .unwrap_or_default()
        .replace('`', "\\u0060");
    format!("```{FENCE}\n{json}\n```\n")
}

fn repro_text(view: &Value) -> Option<(String, String)> {
    let r = view.get("repro").filter(|r| r.is_object())?;
    let script = r["script"].as_str()?.to_string();
    let verdict = match r["verified"].as_bool() {
        Some(true) => "verified",
        Some(false) => "not verified",
        None => "verification unknown",
    };
    Some((script, verdict.to_string()))
}

fn human(view: &Value, blame: &Blame, with_extras: bool) -> String {
    let mut b = String::new();
    b.push_str(&format!(
        "**{}** seen by `{}` {} on {}/{}.\n\n",
        s(view, "kind"),
        s(view, "origin"),
        s(view, "version"),
        s(view, "os"),
        s(view, "arch")
    ));
    b.push_str(&format!(
        "- fingerprint: `{}`\n- report: `{}`\n- blame: `{}` (decided from the {})\n",
        s(view, "fingerprint"),
        s(view, "id"),
        blame.product,
        blame.via.word()
    ));
    if let Some(c) = view["details"]["carton"].as_str() {
        b.push_str(&format!("- carton: `{}`\n", safe(c)));
    }
    if let Some(f) = view["details"]["first_bad_carton"].as_str() {
        b.push_str(&format!("- first bad carton: `{}`\n", safe(f)));
    }
    b.push_str("\n### What happened\n\n```\n");
    b.push_str(&safe(s(view, "panic_message")));
    b.push_str("\n```\n");
    if !s(view, "panic_location").is_empty() {
        b.push_str(&format!("\nat `{}`\n", safe(s(view, "panic_location"))));
    }
    if with_extras {
        if let Some((script, verdict)) = repro_text(view) {
            b.push_str(&format!(
                "\n### Repro ({verdict})\n\n```\n{}\n```\n",
                safe(&script)
            ));
        }
        let frames: Vec<&str> = view["backtrace"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        if !frames.is_empty() {
            b.push_str("\n### Frames\n\n```\n");
            for f in frames {
                b.push_str(&safe(f));
                b.push('\n');
            }
            b.push_str("```\n");
        }
    }
    b
}

/// The issue for a report that has none yet.
pub fn render(view: &Value, blame: &Blame) -> Rendered {
    let block = fenced(&machine_block(view, blame));
    let mut body = format!("{}\n{block}", human(view, blame, true));
    if body.len() > BODY_MAX {
        body = format!("{}\n{block}", human(view, blame, false));
    }
    Rendered {
        title: title(view),
        body,
        labels: labels(view, blame),
    }
}

/// The comment for a report that matches an issue already filed: one more sighting.
pub fn seen_again(view: &Value, blame: &Blame) -> String {
    let mut block = machine_block(view, blame);
    if let Some(o) = block.as_object_mut() {
        // the issue holds the repro; a sighting is counted, not repeated
        o.insert("repro".into(), Value::Null);
    }
    format!(
        "Seen again: `{}` {} on {}/{} (report `{}`).\n\n{}",
        s(view, "origin"),
        s(view, "version"),
        s(view, "os"),
        s(view, "arch"),
        s(view, "id"),
        fenced(&block)
    )
}

/// The line left in the origin's own issue when the fault was blamed on another product.
pub fn pointer_line(view: &Value, blame: &Blame, url: &str) -> String {
    format!(
        "The fault is in `{}` (decided from the {}); filed there: {url} (fingerprint `{}`).",
        blame.product,
        blame.via.word(),
        s(view, "fingerprint")
    )
}

/// The search for the issue a report belongs to: its fingerprint, in one repository, issues only.
pub fn search_query(view: &Value, repo: &str) -> String {
    format!("{} repo:{repo} is:issue in:body", s(view, "fingerprint"))
}

/// What the hive does: the last `cig-report v1` block of a body, parsed and checked.
pub fn parse(body: &str) -> Result<Value, String> {
    let open = format!("```{FENCE}");
    let mut lines = body.lines().collect::<Vec<_>>();
    let start = lines
        .iter()
        .rposition(|l| l.trim_end() == open)
        .ok_or("no cig-report v1 block")?;
    lines.drain(..=start);
    let end = lines
        .iter()
        .position(|l| l.trim_end() == "```")
        .ok_or("the cig-report v1 block is not closed")?;
    let v: Value = serde_json::from_str(&lines[..end].join("\n"))
        .map_err(|e| format!("the cig-report v1 block is not JSON: {e}"))?;
    if v["schema"] != "cig-report" || v["version"] != BLOCK_VERSION {
        return Err("not a cig-report v1 block".into());
    }
    Ok(v)
}

/// The readable part alone, for a link that must stay short.
pub fn human_short(view: &Value, blame: &Blame) -> String {
    human(view, blame, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> Value {
        json!({
            "id": "20261110T010101Z-ab12cd", "fingerprint": "ab12cd34", "kind": "tripwire",
            "origin": "ciginstaller", "version": "0.4.0", "os": "linux", "arch": "x86_64",
            "panic_message": "rollback tripwire: ``` closes a fence", "panic_location": "T-rollback",
            "backtrace": ["ciginstaller::update"],
            "details": {"carton": "2026.11", "regression": true},
            "repro": {"script": "say `hi`\n```\n", "verified": true}
        })
    }

    #[test]
    fn the_block_round_trips_and_the_fence_holds() {
        let v = view();
        let b = crate::crash::blame::of(&v);
        let r = render(&v, &b);
        let parsed = parse(&r.body).unwrap();
        assert_eq!(parsed["fingerprint"], "ab12cd34");
        assert_eq!(parsed["repro"]["script"], "say `hi`\n```\n");
        assert_eq!(parsed, machine_block(&v, &b));
        assert!(r.labels.contains(&"severity:high".to_string()));
        assert!(r.labels.contains(&"regression".to_string()));
        assert!(parse("nothing here").is_err());
    }
}
