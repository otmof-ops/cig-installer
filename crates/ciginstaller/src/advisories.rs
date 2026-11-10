// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! Security advisories (#4, #12). The first install or update that meets one shows it in full,
//! with the severity and the fixed version. A security advisory is never silenced after that:
//! every install and update prints one line, "N security advisories apply: ciginstaller check".
//! Other severities (a bug, a note) are shown once per version and then stay quiet.
//! `ciginstaller check` always prints the whole matrix and never consults this memory.
//!
//! The memory is `advisories_seen` in `state.json`: advisory finding subject (`<id> <tool>
//! <version>`, so a new affected version is a new notice) to the time it was shown. It lives in
//! the state's `extra` map, is written by the same journaled state save as the rest, is only
//! written for what was actually displayed (not under `--quiet` or `--json`), and keeps the
//! newest [`CAP`] entries.

use std::collections::BTreeMap;

use cig_compat::{AdvisorySeverity, Finding, Manifest, E_ADVISORY};
use cigi_home::{now_rfc3339, State};
use serde_json::Value;

pub const KEY: &str = "advisories_seen";

/// The most subjects remembered; the oldest go first.
pub const CAP: usize = 200;

pub fn is_advisory(f: &Finding) -> bool {
    f.code == E_ADVISORY
}

/// Is this advisory finding a security one? The finding's subject begins with the advisory id.
pub fn is_security(m: &Manifest, f: &Finding) -> bool {
    m.advisories
        .iter()
        .any(|a| a.severity == AdvisorySeverity::Security && f.subject.starts_with(&a.id))
}

/// Has this advisory finding been shown already?
pub fn seen(state: &State, f: &Finding) -> bool {
    state
        .extra
        .get(KEY)
        .and_then(|v| v.get(&f.subject))
        .is_some()
}

/// What an install or update does with one advisory finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Triage {
    /// Print it in full, and remember that it was shown.
    Show,
    /// Shown before and not a security advisory: say nothing.
    Skip,
    /// Shown before and a security advisory: counted in the one-line reminder.
    Remind,
}

pub fn triage(m: &Manifest, state: &State, f: &Finding, read_only: bool) -> Triage {
    if read_only || !seen(state, f) {
        Triage::Show
    } else if is_security(m, f) {
        Triage::Remind
    } else {
        Triage::Skip
    }
}

/// The reminder line for `n` security advisories already shown.
pub fn reminder(n: usize) -> String {
    if n == 1 {
        "1 security advisory applies: ciginstaller check".to_string()
    } else {
        format!("{n} security advisories apply: ciginstaller check")
    }
}

/// Remember these findings as shown, keeping the newest [`CAP`]. Call before the state is saved by
/// the transaction, and only when the advisory was actually displayed.
pub fn mark(state: &mut State, findings: &[&Finding]) {
    if findings.is_empty() {
        return;
    }
    let now = now_rfc3339();
    let mut map: BTreeMap<String, Value> = state
        .extra
        .get(KEY)
        .and_then(|v| v.as_object())
        .map(|o| o.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();
    for f in findings {
        map.insert(f.subject.clone(), Value::String(now.clone()));
    }
    cap(&mut map, CAP);
    state.extra.insert(
        KEY.to_string(),
        serde_json::to_value(map).unwrap_or(Value::Null),
    );
}

/// Drop the oldest entries (by the time they were shown) until at most `max` remain.
pub fn cap(map: &mut BTreeMap<String, Value>, max: usize) {
    while map.len() > max {
        let oldest = map
            .iter()
            .min_by(|a, b| a.1.as_str().cmp(&b.1.as_str()).then(a.0.cmp(b.0)))
            .map(|(k, _)| k.clone());
        match oldest {
            Some(k) => {
                map.remove(&k);
            }
            None => break,
        }
    }
}

/// Was this run's output visible? Under `--quiet` or `--json` nothing was shown to a person.
pub fn visible(ui: &cigi_ui::Ui) -> bool {
    !ui.quiet && ui.mode != cigi_ui::Mode::Json
}

/// The line an install or update prints for a finding: the headline with its severity, the
/// advisory's own words, and the fix.
pub fn line(f: &Finding) -> String {
    let mut s = format!("{}: {}", f.code, f.headline);
    if is_advisory(f) {
        for d in &f.detail {
            s.push_str(&format!(" ({d})"));
        }
        if let Some(fix) = f.fixes.first() {
            s.push_str(&format!("; {fix}"));
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_map_keeps_the_newest_entries() {
        let mut m: BTreeMap<String, Value> = BTreeMap::new();
        for i in 0..10 {
            m.insert(
                format!("a{i}"),
                Value::String(format!("2026-11-{:02}T00:00:00Z", i + 1)),
            );
        }
        cap(&mut m, 4);
        assert_eq!(m.len(), 4);
        assert!(m.contains_key("a9") && m.contains_key("a6") && !m.contains_key("a5"));
    }

    #[test]
    fn the_reminder_words() {
        assert_eq!(
            reminder(1),
            "1 security advisory applies: ciginstaller check"
        );
        assert_eq!(
            reminder(3),
            "3 security advisories apply: ciginstaller check"
        );
    }
}
