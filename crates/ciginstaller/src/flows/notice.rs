// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `auto_update = notify` (SPEC 12.2): after an interactive command, at most one line a day says
//! a newer carton is out. No background process: the check happens on invocation, a manifest
//! fetch at most once a day, and `installer/notice.json` remembers the day.

use std::path::PathBuf;

use cig_compat::Version;
use cigi_home::now_rfc3339;
use cigi_source::fetch::cache_age_secs;
use serde_json::{json, Value};

use crate::cx::Cx;

fn path(cx: &Cx) -> PathBuf {
    cx.home.installer().join("notice.json")
}

#[derive(Default)]
struct Notice {
    /// The day a manifest was last asked for (`YYYY-MM-DD`).
    checked: Option<String>,
    /// The day a notice was last shown.
    noticed: Option<String>,
    carton: Option<String>,
}

fn read(cx: &Cx) -> Notice {
    let Some(v) = std::fs::read_to_string(path(cx))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
    else {
        return Notice::default();
    };
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
    Notice {
        checked: s("checked"),
        noticed: s("noticed"),
        carton: s("carton"),
    }
}

/// Written beside the other installer files, atomically. A home with no `installer/` yet gets
/// nothing (the first command that makes one is not a notice's to create).
fn write(cx: &Cx, n: &Notice) {
    let p = path(cx);
    let Some(dir) = p.parent() else { return };
    if !dir.is_dir() {
        return;
    }
    let body = json!({
        "format": 1, "checked": n.checked, "noticed": n.noticed, "carton": n.carton,
    });
    let tmp = dir.join(format!(".notice.json.{}", std::process::id()));
    let text = format!(
        "{}\n",
        serde_json::to_string_pretty(&body).unwrap_or_default()
    );
    if std::fs::write(&tmp, text).is_ok() && std::fs::rename(&tmp, &p).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

fn today() -> String {
    now_rfc3339().chars().take(10).collect()
}

/// Call once a command has finished well. Quiet unless the command is interactive (a terminal on
/// stdout and stderr, no `--json`, no `--quiet`, no `--offline`) and a newer carton is out.
pub fn maybe_notify(cx: &Cx) {
    let cfg_on = cx.cfg.get("update_check") != "off" && cx.cfg.get("auto_update") != "off";
    let interactive = cx.ui.tty_out
        && cx.ui.tty_err
        && cx.ui.mode != cigi_ui::Mode::Json
        && !cx.ui.quiet
        && !cx.globals.offline;
    if !cfg_on || !interactive {
        return;
    }
    let Some(machine) = cx
        .state
        .machine_carton
        .as_deref()
        .and_then(|m| Version::parse(m).ok())
    else {
        return;
    };
    let day = today();
    let mut n = read(cx);
    if n.noticed.as_deref() == Some(day.as_str()) {
        return;
    }
    let channel = cx.channel();
    let fresh =
        cache_age_secs(&cx.home, &channel, None).is_some_and(|a| (0..24 * 3600).contains(&a));
    let manifest = if fresh || n.checked.as_deref() == Some(day.as_str()) {
        cx.peek_manifest()
    } else {
        // The fetch writes the manifest cache: it takes the installer lock without waiting, and
        // the notice is skipped when another installer holds it (or this command still does).
        let _guard = match cx.lock_ref() {
            Ok(_) => None,
            Err(_) => match cx.home.lock_with(std::time::Duration::ZERO, &mut |_| {}) {
                Ok(l) => Some(l),
                Err(_) => return,
            },
        };
        n.checked = Some(day.clone());
        write(cx, &n);
        cx.fetch_manifest(&channel)
            .ok()
            .map(|a| a.get().clone())
            .or_else(|| cx.peek_manifest())
    };
    let Some(newest) = manifest.and_then(|m| m.latest(false).map(|c| c.version.clone())) else {
        return;
    };
    if newest <= machine {
        return;
    }
    cx.ui.notice(&format!(
        "carton {newest} is out (this machine is on {machine}); run: ciginstaller update"
    ));
    n.noticed = Some(day);
    n.carton = Some(newest.to_string());
    write(cx, &n);
}
