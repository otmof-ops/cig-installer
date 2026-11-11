// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `config`, `config get <key>`, `config set <key> <value>`, `config unset <key>` (SPEC 5.2, 10).
//! The value in force is a machine value that is locked, then the environment, then the file, then
//! the machine's unlocked value, then the default (#17). `set` and `unset` go through the
//! journal; a key or value outside 5.2 is `E1870`, `unset` of a key the file does not hold is
//! `E1875`, and either on a key the machine locks is `E1904`, which names the store.

use cigi_home::{Config, KeySpec, KEYS};
use cigi_ui::{Align, Cell, InstallerError, Report, Table};
use serde_json::{json, Map, Value};

use super::tabular::details;
use super::Outcome;
use crate::cli::ConfigSub;
use crate::cx::Cx;
use crate::errors::ie;
use crate::flows::settle;

pub fn run(cx: &mut Cx, sub: Option<&ConfigSub>) -> Result<Outcome, InstallerError> {
    match sub {
        None | Some(ConfigSub::List) => list(cx),
        Some(ConfigSub::Get { key }) => get(cx, key),
        Some(ConfigSub::Set { key, value }) => set(cx, key, value),
        Some(ConfigSub::Unset { key }) => unset(cx, key),
    }
}

fn known(key: &str) -> Result<&'static KeySpec, InstallerError> {
    KEYS.iter().find(|k| k.key == key).ok_or_else(|| {
        InstallerError::new(
            "E1870",
            format!(
                "there is no setting `{key}`; the settings are {}",
                KEYS.iter().map(|k| k.key).collect::<Vec<_>>().join(", ")
            ),
        )
        .with_fix("ciginstaller config")
    })
}

fn shown(v: &str) -> &str {
    if v.is_empty() {
        "(unset)"
    } else {
        v
    }
}

fn env_name(key: &str) -> String {
    format!("CIGI_{}", key.to_ascii_uppercase())
}

/// `locked by /etc/cigscript/config.toml`, or `None` when nothing locks the key.
pub fn locked_text(cfg: &Config, key: &str) -> Option<String> {
    cfg.locked_by(key).map(|s| format!("locked by {s}"))
}

/// Where a value comes from, in words: the lock, the environment variable, the file, the machine
/// layer's unlocked value, the default.
fn from_text(cfg: &Config, key: &str) -> String {
    match cfg.source(key) {
        "locked" => locked_text(cfg, key).unwrap_or_default(),
        "environment" => format!("environment ({})", env_name(key)),
        "machine" => format!(
            "machine ({})",
            cfg.machine_value(key).map(|(_, s)| s).unwrap_or_default()
        ),
        s => s.to_string(),
    }
}

/// The refusal for a key the machine locks.
fn refuse_locked(cfg: &Config, key: &str) -> Result<(), InstallerError> {
    cfg.check_unlocked(key).map_err(|e| {
        ie(e)
            .with_fix(format!("ciginstaller config get {key}"))
            .with_exit(cigi_ui::EXIT_REFUSED)
    })
}

fn setting(cfg: &Config, k: &KeySpec) -> Value {
    json!({
        "key": k.key,
        "value": cfg.get(k.key),
        "source": cfg.source(k.key),
        "locked": cfg.locked_by(k.key).is_some(),
        "locked_by": cfg.locked_by(k.key),
        "default": k.default,
        "allowed": k.values,
        "range": Config::numeric_range(k.key),
        "note": (k.key == "preset").then_some(super::install::PRESET_NOTE),
    })
}

fn list(cx: &mut Cx) -> Result<Outcome, InstallerError> {
    let mut table = Table::new(&[
        ("setting", Align::Left),
        ("from", Align::Left),
        ("value", Align::Left),
    ]);
    let mut config = Map::new();
    let mut settings = Vec::new();
    let mut moved = 0;
    for k in KEYS {
        let value = cx.cfg.get(k.key);
        let source = cx.cfg.source(k.key);
        let cell = if source == "default" {
            Cell::new(shown(&value))
        } else {
            moved += 1;
            Cell::changed(shown(&value))
        };
        let from = from_text(&cx.cfg, k.key);
        // the value goes last: a long one (a mirror URL) is the column that may be cut short
        table = table.row(vec![Cell::new(k.key), Cell::new(from), cell]);
        config.insert(k.key.to_string(), Value::String(value));
        settings.push(setting(&cx.cfg, k));
    }
    let mut r = Report::done(format!(
        "{} settings, {moved} not at their default",
        KEYS.len()
    ));
    for l in details(&cx.ui, &table) {
        r = r.detail(l);
    }
    r = r.detail(format!("preset: {}", super::install::PRESET_NOTE));
    Ok(r.next("ciginstaller config set <setting> <value>")
        .data(json!({ "config": config, "settings": settings }))
        .into())
}

fn get(cx: &mut Cx, key: &str) -> Result<Outcome, InstallerError> {
    let k = known(key)?;
    let value = cx.cfg.get(key);
    let source = cx.cfg.source(key);
    let mut r = Report::done(format!("{key} = {}", shown(&value)));
    r = r.detail(match source {
        "locked" => format!(
            "{}; nothing on this machine can change it",
            locked_text(&cx.cfg, key).unwrap_or_default()
        ),
        "environment" => format!("from the environment ({})", env_name(key)),
        "file" => "from the config file".to_string(),
        "machine" => format!(
            "from the machine layer ({}); your own config file outranks it",
            cx.cfg
                .machine_value(key)
                .map(|(_, s)| s)
                .unwrap_or_default()
        ),
        _ => "the default; nothing sets it".to_string(),
    });
    if key == "preset" {
        r = r.detail(super::install::PRESET_NOTE);
    }
    Ok(r.data(json!({
        "config": { key: value },
        "key": key,
        "value": value,
        "source": source,
        "locked": cx.cfg.locked_by(key).is_some(),
        "locked_by": cx.cfg.locked_by(key),
        "default": k.default,
    }))
    .into())
}

fn set(cx: &mut Cx, key: &str, value: &str) -> Result<Outcome, InstallerError> {
    known(key)?;
    Config::validate(key, value).map_err(ie)?;
    refuse_locked(&cx.cfg, key)?;
    // the layout and the lock first; the file is read again under the lock so a concurrent
    // edit is not overwritten with an older copy
    cx.lock_and_recover()?;
    cx.cfg = Config::load(&cx.home).map_err(ie)?;
    let previous = cx.cfg.get_file(key);
    let changed = previous.as_deref() != Some(value);
    if changed {
        let mut txn = cx.begin(&format!("config set {key}"))?;
        let written = cx.cfg.set_in(&mut txn, key, value).map_err(ie);
        settle(txn, written)?;
    }
    let mut r = Report::done(if changed {
        format!("{key} = {}", shown(value))
    } else {
        format!("{key} was already {}", shown(value))
    });
    if cx.cfg.env_has(key) {
        r = r.detail(format!(
            "{} is set in this shell and outranks the file",
            env_name(key)
        ));
    }
    if key == "channel" && changed {
        r = r.detail(
            "nothing was fetched; ciginstaller channel <name> checks the channel's manifest first",
        );
    }
    Ok(r.data(json!({
        "config": { key: cx.cfg.get(key) },
        "key": key,
        "value": value,
        "previous": previous,
        "changed": changed,
    }))
    .into())
}

fn unset(cx: &mut Cx, key: &str) -> Result<Outcome, InstallerError> {
    known(key)?;
    refuse_locked(&cx.cfg, key)?;
    cx.lock_and_recover()?;
    cx.cfg = Config::load(&cx.home).map_err(ie)?;
    let Some(previous) = cx.cfg.get_file(key) else {
        let from = match cx.cfg.source(key) {
            "environment" => format!("it comes from {}", env_name(key)),
            _ => "it is at its default".to_string(),
        };
        return Err(InstallerError::new(
            "E1875",
            format!("{key} is not in the config file; {from}"),
        )
        .with_fix(format!("ciginstaller config get {key}")));
    };
    let mut txn = cx.begin(&format!("config unset {key}"))?;
    let done = cx.cfg.unset_in(&mut txn, key).map_err(ie);
    settle(txn, done)?;
    let now = cx.cfg.get(key);
    let mut r = Report::done(format!("{key} is back to {}", shown(&now)));
    if cx.cfg.env_has(key) {
        r = r.detail(format!(
            "{} is set in this shell and outranks the file",
            env_name(key)
        ));
    }
    Ok(r.data(json!({
        "config": { key: now },
        "key": key,
        "value": now,
        "previous": previous,
        "source": cx.cfg.source(key),
    }))
    .into())
}
