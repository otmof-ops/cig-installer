// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0
//! `which <tool> [--why] [--path]`.

use std::path::Path;

use cigi_home::{now_rfc3339, State};
use cigi_ui::{InstallerError, Report, EXIT_REFUSED};
use serde_json::json;

use super::Outcome;
use crate::cx::Cx;
use crate::resolve::{resolve, Mode, Resolution};

pub fn run(cx: &mut Cx, tool: &str, why: bool, path_only: bool) -> Result<Outcome, InstallerError> {
    let res = resolve(cx, tool, Mode::Which).map_err(|mut e| {
        e.exit = EXIT_REFUSED;
        e
    })?;
    touch_last_used(cx, &res);
    if path_only {
        return Ok(Outcome::Page(res.path.display().to_string()));
    }
    let mut r = Report::done(res.text(tool)).detail(res.path.display().to_string());
    if why {
        for s in &res.steps {
            r = r.detail(s.clone());
        }
    }
    Ok(r.data(json!({
        "tool": tool,
        "version": res.build.to_string(),
        "carton": res.carton.as_ref().map(|c| c.to_string()),
        "path": res.path.display().to_string(),
        "steps": res.steps,
    }))
    .into())
}

/// `last_used` moves at most once a day and only when the lock is free; losing the race is fine.
pub fn touch_last_used(cx: &Cx, res: &Resolution) {
    let Some(c) = &res.carton else { return };
    let key = c.to_string();
    let today = now_rfc3339();
    let day = today.get(..10).unwrap_or("");
    let stale = cx
        .state
        .cartons
        .get(&key)
        .is_some_and(|r| r.last_used.as_deref().and_then(|l| l.get(..10)) != Some(day));
    if !stale {
        return;
    }
    let Ok(_lock) = cx.home.lock_with(std::time::Duration::ZERO, &mut |_| {}) else {
        return;
    };
    let Ok(mut state) = State::load(&cx.home) else {
        return;
    };
    let Some(rec) = state.cartons.get_mut(&key) else {
        return;
    };
    rec.last_used = Some(today);
    let _ = write_atomic(&cx.home.state_path(), state.to_json().as_bytes());
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_file_name(format!(".state.json.tmp-{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}
