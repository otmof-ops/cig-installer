// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! Security advisories are shown once per version (#4, #12): the first install or update that
//! meets one says so, with the severity and the fixed version; after that it stays quiet.
//! `ciginstaller check` always prints the whole matrix and never consults this memory.
//!
//! The memory is `advisories_seen` in `state.json`: advisory finding subject (`<id> <tool>
//! <version>`, so a new affected version is a new notice) to the time it was shown. It lives in
//! the state's `extra` map and is written by the same journaled state save as the rest.

use std::collections::BTreeMap;

use cig_compat::{Finding, E_ADVISORY};
use cigi_home::{now_rfc3339, State};
use serde_json::Value;

pub const KEY: &str = "advisories_seen";

pub fn is_advisory(f: &Finding) -> bool {
    f.code == E_ADVISORY
}

/// Has this advisory finding been shown already?
pub fn seen(state: &State, f: &Finding) -> bool {
    state
        .extra
        .get(KEY)
        .and_then(|v| v.get(&f.subject))
        .is_some()
}

/// Remember these findings as shown. Call before the state is saved by the transaction.
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
    state.extra.insert(
        KEY.to_string(),
        serde_json::to_value(map).unwrap_or(Value::Null),
    );
}

/// The line an install or update prints for a finding: the headline with its severity, the
/// advisory's own words, and the command that fixes it.
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
