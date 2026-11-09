// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0

//! `installer/state.json` (section 5.1).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::error::StateError;
use crate::home::Home;
use crate::journal::Txn;

/// State format this build writes and reads.
pub const STATE_FORMAT: u32 = 1;

/// `history` keeps this many entries.
pub const HISTORY_MAX: usize = 50;

/// One installed carton.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CartonRecord {
    pub installed_at: String,
    pub tools: BTreeMap<String, String>,
    pub reason: String,
    #[serde(default)]
    pub last_used: Option<String>,
    /// Set by `uninstall <tool>`: the carton lost a tool.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub partial: bool,
}

/// A single tool installed outside the machine carton's set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stick {
    pub tool: String,
    pub version: String,
    pub from_carton: String,
    pub installed_at: String,
    pub reason: String,
}

/// Where `rollback` goes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Previous {
    pub machine_carton: Option<String>,
    pub kept_until: Option<String>,
}

/// The installer's own record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallerInfo {
    pub version: String,
    pub previous: Option<String>,
    pub healthy: bool,
    pub first_ok_run: Option<String>,
}

/// The last admitted manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestInfo {
    pub sequence: u64,
    pub channel: String,
    pub fetched_at: String,
    pub expires: String,
    pub trust_level: String,
}

/// One past operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: String,
    pub at: String,
    pub what: String,
    pub from: Option<String>,
    pub to: Option<String>,
    pub txn: String,
    pub ok: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GcInfo {
    pub last_run: Option<String>,
}

/// `state.json`. Unknown keys are kept in `extra` and written back after the
/// known ones, so a newer installer's additions survive.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub format: u32,
    pub channel: String,
    pub machine_carton: Option<String>,
    pub cartons: BTreeMap<String, CartonRecord>,
    pub sticks: Vec<Stick>,
    pub previous: Previous,
    pub installer: InstallerInfo,
    pub manifest: Option<ManifestInfo>,
    pub history: Vec<HistoryEntry>,
    pub gc: GcInfo,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl Default for State {
    fn default() -> State {
        State {
            format: STATE_FORMAT,
            channel: "stable".to_string(),
            machine_carton: None,
            cartons: BTreeMap::new(),
            sticks: Vec::new(),
            previous: Previous::default(),
            installer: InstallerInfo {
                version: env!("CARGO_PKG_VERSION").to_string(),
                previous: None,
                healthy: false,
                first_ok_run: None,
            },
            manifest: None,
            history: Vec::new(),
            gc: GcInfo::default(),
            extra: BTreeMap::new(),
        }
    }
}

impl State {
    /// Read `state.json`; a home with none yet gets the default (nothing
    /// installed). A newer `format` or unreadable JSON is `E1830`.
    pub fn load(home: &Home) -> Result<State, StateError> {
        let p = home.state_path();
        let text = match std::fs::read_to_string(&p) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(State::default()),
            Err(source) => return Err(StateError::Io { path: p, source }),
        };
        State::parse(&text).map_err(|why| StateError::Unreadable { path: p, why })
    }

    /// Parse the text of a `state.json`; the error is the reason.
    pub fn parse(text: &str) -> Result<State, String> {
        let val: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let found = val.get("format").and_then(|f| f.as_u64()).unwrap_or(0);
        if found == 0 || found > STATE_FORMAT as u64 {
            return Err(format!("format {found}, this build knows {STATE_FORMAT}"));
        }
        serde_json::from_value(val).map_err(|e| e.to_string())
    }

    /// Two-space pretty JSON with a trailing newline.
    pub fn to_json(&self) -> String {
        let mut s = serde_json::to_string_pretty(self).unwrap_or_default();
        s.push('\n');
        s
    }

    /// Write through the journal (saved copy, atomic rename).
    pub fn save_in(&self, txn: &mut Txn) -> Result<(), StateError> {
        let p = txn.home().state_path();
        txn.write(&p, self.to_json().as_bytes(), Some(0o644))?;
        Ok(())
    }

    /// Append to `history`, keeping the last [`HISTORY_MAX`].
    pub fn push_history(&mut self, entry: HistoryEntry) {
        self.history.push(entry);
        if self.history.len() > HISTORY_MAX {
            let drop = self.history.len() - HISTORY_MAX;
            self.history.drain(..drop);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
  "format": 1,
  "channel": "stable",
  "machine_carton": "0.1.0",
  "cartons": {
    "0.1.0": { "installed_at": "2026-11-12T10:03:11Z", "tools": { "cig": "1.20.0", "cigo": "1.17.0", "cigcompile": "0.2.0" },
               "reason": "install carton", "last_used": "2026-11-13T08:00:00Z" }
  },
  "sticks": [ { "tool": "cigo", "version": "1.16.0", "from_carton": "0.0.9", "installed_at": "2026-11-12T10:03:11Z", "reason": "pin cigo" } ],
  "previous": { "machine_carton": null, "kept_until": null },
  "installer": { "version": "0.1.0", "previous": null, "healthy": true, "first_ok_run": "2026-11-12T10:05:00Z" },
  "manifest": { "sequence": 7, "channel": "stable", "fetched_at": "2026-11-12T10:00:00Z", "expires": "2026-12-12T10:00:00Z", "trust_level": "checksummed" },
  "history": [ { "id": "20261112T100311-a1b2c3", "at": "2026-11-12T10:03:11Z", "what": "install", "from": null, "to": "0.1.0", "txn": "20261112T100311-a1b2c3", "ok": true } ],
  "gc": { "last_run": null },
  "future_key": { "x": 1 }
}"#;

    #[test]
    fn the_spec_sample_parses_and_round_trips() {
        let s = State::parse(SAMPLE).unwrap();
        assert_eq!(s.machine_carton.as_deref(), Some("0.1.0"));
        assert_eq!(s.cartons["0.1.0"].tools["cigo"], "1.17.0");
        assert!(s.extra.contains_key("future_key"));
        let text = s.to_json();
        assert!(text.ends_with("}\n"));
        assert_eq!(State::parse(&text).unwrap(), s);
        // keys in the documented order
        let order: Vec<usize> = [
            "\"format\"",
            "\"channel\"",
            "\"machine_carton\"",
            "\"cartons\"",
            "\"sticks\"",
            "\"previous\"",
            "\"installer\"",
            "\"manifest\"",
            "\"history\"",
            "\"gc\"",
        ]
        .iter()
        .map(|k| text.find(k).unwrap())
        .collect();
        assert!(order.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn too_new_and_garbage_are_refused() {
        assert!(State::parse(&SAMPLE.replace("\"format\": 1", "\"format\": 2")).is_err());
        assert!(State::parse("{").is_err());
    }

    #[test]
    fn history_is_capped() {
        let mut s = State::default();
        for i in 0..60 {
            s.push_history(HistoryEntry {
                id: i.to_string(),
                at: String::new(),
                what: "x".into(),
                from: None,
                to: None,
                txn: String::new(),
                ok: true,
            });
        }
        assert_eq!(s.history.len(), HISTORY_MAX);
        assert_eq!(s.history[0].id, "10");
    }
}
