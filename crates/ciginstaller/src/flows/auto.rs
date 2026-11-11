// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `auto_update = security | all` (SPEC 12.2, #7): `ciginstaller update --auto`, what the
//! background schedule runs.
//!
//! - `security` installs the newest carton whose builds clear at least one security advisory that
//!   applies to the carton this machine is on. The advisory is the manifest's own `[[advisory]]`
//!   entry, read from the manifest the installer admitted (signed, when the channel is under TUF).
//! - `all` installs the newest carton.
//! - Both are the ordinary update, trial included: a carton that fails its trial (or any step) is
//!   rolled back, the old carton stays, and the outcome is kept in `installer/auto-update.json`
//!   for the next terminal to say. The same carton is not tried again for [`RETRY_DAYS`] days.
//! - At most one attempt a day, and none on battery power. The schedule adds the idle and jitter
//!   conditions its OS offers (`flows/schedule.rs`).
//! - `auto_update` is read through the configuration, so a machine-locked value (#17) is the one
//!   obeyed here; nothing in this module writes it.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use cig_compat::{check, Carton, Combination, Host, Manifest, Origin, Severity, Version};
use cigi_ui::{InstallerError, Report};
use serde_json::{json, Value};

use super::update::{self, UpdateArgs};
use crate::advisories::{is_advisory, is_security};
use crate::cx::{Cx, ManifestMode};

/// A carton that failed is not tried again for this long (a newer carton is tried at once).
pub const RETRY_DAYS: u64 = 7;
/// An attempt this recent blocks another: a day, less four hours so a daily timer with jitter
/// is never skipped by its own previous run.
pub const MIN_GAP_SECS: u64 = 20 * 3600;

#[derive(Debug, Clone, Default)]
pub struct AutoArgs {
    pub project: Option<PathBuf>,
}

fn path(cx: &Cx) -> PathBuf {
    cx.home.installer().join("auto-update.json")
}

fn epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// What the last attempts left behind.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Memory {
    pub last_attempt: Option<u64>,
    /// `updated`, `rolled_back` or `nothing`.
    pub result: Option<String>,
    pub carton: Option<String>,
    pub message: Option<String>,
    pub shown: bool,
    /// The carton that failed and when (epoch seconds).
    pub failed: Option<(String, u64)>,
}

pub fn read(cx: &Cx) -> Memory {
    let Some(v) = std::fs::read_to_string(path(cx))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
    else {
        return Memory::default();
    };
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
    Memory {
        last_attempt: v.get("last_attempt").and_then(|x| x.as_u64()),
        result: s("result"),
        carton: s("carton"),
        message: s("message"),
        shown: v.get("shown").and_then(|x| x.as_bool()).unwrap_or(true),
        failed: v.get("failed").and_then(|f| {
            Some((
                f.get("carton")?.as_str()?.to_string(),
                f.get("at")?.as_u64()?,
            ))
        }),
    }
}

/// Written beside the other installer files, atomically; a home with no `installer/` gets nothing.
pub fn write(cx: &Cx, m: &Memory) {
    let p = path(cx);
    let Some(dir) = p.parent() else { return };
    if !dir.is_dir() {
        return;
    }
    let body = json!({
        "format": 1, "last_attempt": m.last_attempt, "result": m.result, "carton": m.carton,
        "message": m.message, "shown": m.shown,
        "failed": m.failed.as_ref().map(|(c, at)| json!({"carton": c, "at": at})),
    });
    let tmp = dir.join(format!(".auto-update.json.{}", std::process::id()));
    let text = format!(
        "{}\n",
        serde_json::to_string_pretty(&body).unwrap_or_default()
    );
    if std::fs::write(&tmp, text).is_ok() && std::fs::rename(&tmp, &p).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// The line the next terminal prints once about an automatic update that was rolled back.
pub fn pending_notice(cx: &Cx) -> Option<String> {
    let m = read(cx);
    if m.shown || m.result.as_deref() != Some("rolled_back") {
        return None;
    }
    Some(format!(
        "the automatic update to carton {} was rolled back and this machine stayed as it was: {}; see: ciginstaller update --trial-only",
        m.carton.as_deref().unwrap_or("?"),
        m.message.as_deref().unwrap_or("it failed")
    ))
}

pub fn mark_notice_shown(cx: &Cx) {
    let mut m = read(cx);
    if !m.shown {
        m.shown = true;
        write(cx, &m);
    }
}

/// Is the machine on mains power? Reads a `power_supply` directory: when any supply of type
/// `Mains` is online, or there is none (a desktop), yes; on battery alone, no. Other systems
/// answer yes here: their schedules enforce AC power themselves (`DisallowStartIfOnBatteries`,
/// `ProcessType`), and the check on this side only adds to it.
pub fn on_ac(root: &Path) -> bool {
    let Ok(rd) = std::fs::read_dir(root) else {
        return true;
    };
    let mut mains = 0;
    for e in rd.flatten() {
        let d = e.path();
        let kind = std::fs::read_to_string(d.join("type")).unwrap_or_default();
        if kind.trim().eq_ignore_ascii_case("mains") {
            mains += 1;
            if std::fs::read_to_string(d.join("online")).is_ok_and(|o| o.trim() == "1") {
                return true;
            }
        }
    }
    mains == 0
}

fn power_root() -> PathBuf {
    // a test points this at a folder it writes; there is no other use
    std::env::var_os("CIGI_TEST_POWER_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/sys/class/power_supply"))
}

fn skipped(why: impl Into<String>) -> Result<Report, InstallerError> {
    let why = why.into();
    Ok(Report::done(format!("auto update: {why}")).data(json!({ "auto": true, "skipped": why })))
}

/// Security advisories that apply to a combination, by id.
fn security_ids(
    m: &Manifest,
    items: &[(String, Version)],
    carton: &Version,
    host: &Host,
) -> Vec<String> {
    let mut ids: Vec<String> = check(
        m,
        &Combination {
            items: items.to_vec(),
            origin: Origin::Carton(carton.clone()),
        },
        host,
    )
    .iter()
    .filter(|f| is_advisory(f) && is_security(m, f))
    .filter_map(|f| f.subject.split(' ').next().map(str::to_string))
    .collect();
    ids.sort();
    ids.dedup();
    ids
}

fn items_of(c: &Carton) -> Vec<(String, Version)> {
    c.products
        .iter()
        .filter(|(t, _)| t.as_str() != "ciginstaller")
        .map(|(t, v)| (t.clone(), v.clone()))
        .collect()
}

/// The carton `auto_update = security` takes: the newest carton, newer than `cur`, not yanked, not
/// a prerelease and with nothing refused, that clears at least one security advisory that applies
/// to the carton the machine is on (`cur_items`). Also the advisory ids it clears.
pub fn security_target<'m>(
    m: &'m Manifest,
    cur: &Version,
    cur_items: &[(String, Version)],
    host: &Host,
) -> Option<(&'m Carton, Vec<String>)> {
    let now = security_ids(m, cur_items, cur, host);
    if now.is_empty() {
        return None;
    }
    m.cartons
        .values()
        .rev()
        .filter(|c| {
            c.version > *cur && !c.version.is_prerelease() && !m.is_carton_yanked(&c.version)
        })
        .find_map(|c| {
            let items = items_of(c);
            let refused = check(
                m,
                &Combination {
                    items: items.clone(),
                    origin: Origin::Carton(c.version.clone()),
                },
                host,
            )
            .iter()
            .any(|f| f.severity == Severity::Refuse);
            if refused {
                return None;
            }
            let after = security_ids(m, &items, &c.version, host);
            let cleared: Vec<String> = now.iter().filter(|i| !after.contains(i)).cloned().collect();
            (!cleared.is_empty()).then_some((c, cleared))
        })
}

/// What the machine is on, as items: the manifest's carton when it still lists it, else the
/// state's record of what was installed.
fn current_items(cx: &Cx, m: &Manifest, cur: &Version) -> Vec<(String, Version)> {
    if let Some(c) = m.carton(cur) {
        return items_of(c);
    }
    cx.state
        .cartons
        .get(&cur.to_string())
        .map(|r| {
            r.tools
                .iter()
                .filter(|(t, _)| t.as_str() != "ciginstaller")
                .filter_map(|(t, v)| Some((t.clone(), Version::parse(v).ok()?)))
                .collect()
        })
        .unwrap_or_default()
}

pub fn run(cx: &mut Cx, a: &AutoArgs) -> Result<Report, InstallerError> {
    let policy = cx.cfg.get("auto_update");
    if policy != "security" && policy != "all" {
        return skipped(format!(
            "auto_update = {policy}; only `security` and `all` install anything in the background"
        ));
    }
    if cx.cfg.get("update_check") == "off" {
        return skipped("update_check = off sends no request at all");
    }
    if cx.globals.offline {
        return skipped("offline");
    }
    let mut mem = read(cx);
    if let Some(t) = mem.last_attempt {
        if epoch().saturating_sub(t) < MIN_GAP_SECS {
            return skipped("tried less than a day ago");
        }
    }
    if !on_ac(&power_root()) {
        return skipped("on battery power; it runs again on the next schedule");
    }
    let Some(machine) = cx.state.machine_carton.clone() else {
        return skipped("nothing is installed");
    };
    let cur = Version::parse(&machine).map_err(|e| {
        crate::errors::bug(format!(
            "the machine carton `{machine}` does not parse: {e}"
        ))
    })?;
    cx.lock_and_recover()?;
    let m: Manifest = cx.manifest(ManifestMode::Fresh)?.clone();

    // `rollout = N%`: a machine outside the share waits for its turn, as `notify` does
    let standing = cigi_home::rollout::standing(&cx.cfg.get("rollout"));
    let (target, why) = if policy == "all" {
        match m.latest(false) {
            Some(c) if c.version > cur => (c.clone(), "auto_update = all".to_string()),
            _ => return record_nothing(cx, mem, "this machine is on the newest carton"),
        }
    } else {
        match security_target(&m, &cur, &current_items(cx, &m, &cur), &cx.host) {
            Some((c, ids)) => (
                c.clone(),
                format!("auto_update = security: it clears {}", ids.join(", ")),
            ),
            None => return record_nothing(cx, mem, "no newer carton clears a security advisory"),
        }
    };
    if !standing.in_share {
        return skipped(format!(
            "carton {} is rolling out, {} of machines so far; this machine waits its turn",
            target.version,
            standing.share_text()
        ));
    }
    if let Some((failed, at)) = &mem.failed {
        if *failed == target.version.to_string()
            && epoch().saturating_sub(*at) < RETRY_DAYS * 24 * 3600
        {
            return skipped(format!(
                "carton {failed} failed its trial recently and was rolled back; it is not tried again for {RETRY_DAYS} days unless a newer carton comes out"
            ));
        }
    }

    let args = UpdateArgs {
        to: Some(target.version.to_string()),
        project: a.project.clone(),
        ..UpdateArgs::default()
    };
    let result = update::run(cx, &args);
    mem.last_attempt = Some(epoch());
    mem.carton = Some(target.version.to_string());
    match result {
        Ok(u) => {
            mem.result = Some("updated".into());
            mem.message = Some(why.clone());
            mem.shown = true;
            mem.failed = None;
            write(cx, &mem);
            Ok(u.report
                .detail(format!("installed automatically: {why}"))
                .data(json!({ "auto": true, "carton": target.version.to_string(), "reason": why })))
        }
        Err(e) => {
            // the update's own transaction rolled everything back before this error came out
            mem.result = Some("rolled_back".into());
            mem.message = Some(format!("{} {}", e.code, e.message));
            mem.shown = false;
            mem.failed = Some((target.version.to_string(), epoch()));
            write(cx, &mem);
            Err(e.with_hint(format!(
                "automatic update to carton {} failed and was rolled back; carton {cur} stays",
                target.version
            )))
        }
    }
}

fn record_nothing(cx: &Cx, mut mem: Memory, why: &str) -> Result<Report, InstallerError> {
    mem.last_attempt = Some(epoch());
    mem.result = Some("nothing".into());
    mem.message = Some(why.into());
    write(cx, &mem);
    skipped(why)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn supply(root: &Path, name: &str, kind: &str, online: &str) {
        let d = root.join(name);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("type"), format!("{kind}\n")).unwrap();
        std::fs::write(d.join("online"), format!("{online}\n")).unwrap();
    }

    #[test]
    fn power_is_read_from_the_mains_supplies() {
        let d = tempfile::tempdir().unwrap();
        // no power_supply folder, and a desktop with none: the schedule decides
        assert!(on_ac(&d.path().join("absent")));
        assert!(on_ac(d.path()));
        supply(d.path(), "BAT0", "Battery", "0");
        assert!(on_ac(d.path()), "a battery alone says nothing about mains");
        supply(d.path(), "AC", "Mains", "0");
        assert!(!on_ac(d.path()));
        supply(d.path(), "AC", "Mains", "1");
        assert!(on_ac(d.path()));
    }
}
