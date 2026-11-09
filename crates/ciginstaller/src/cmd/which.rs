// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0
//! `which <tool> [--why] [--path]`.

use std::path::Path;

use cigi_home::journal::{list_txns, TxnStatus};
use cigi_home::{now_rfc3339, State};
use cigi_ui::{InstallerError, Report, EXIT_REFUSED};
use serde_json::json;

use super::Outcome;
use crate::cx::Cx;
use crate::resolve::{resolve, Mode, Resolution};

pub fn run(cx: &mut Cx, tool: &str, why: bool, path_only: bool) -> Result<Outcome, InstallerError> {
    // Exit rule: only the codes that mean "refused" (a pin or CIGI_CARTON the machine cannot
    // meet: E1816, E1842, E1801) exit 2. Everything else keeps its own code: E1840 (no tool) is
    // 127, E1831 and the rest follow the registry, E1890 (a bug) is 70.
    let res = resolve(cx, tool, Mode::Which).map_err(|mut e| {
        if matches!(e.code, "E1816" | "E1842" | "E1801") {
            e.exit = EXIT_REFUSED;
        }
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

/// `last_used` moves at most once a day, only when the lock is free, and never while a journal
/// is unfinished: `recover` owns `state.json` then, and a write here would be undone or would
/// disagree with the journal's recorded hash. Losing the race is fine.
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
    if list_txns(&cx.home)
        .iter()
        .any(|t| matches!(t.status, TxnStatus::Open | TxnStatus::Unreadable))
    {
        return;
    }
    let Ok(mut state) = State::load(&cx.home) else {
        return;
    };
    let Some(rec) = state.cartons.get_mut(&key) else {
        return;
    };
    rec.last_used = Some(today);
    let _ = write_atomic(&cx.home.state_path(), state.to_json().as_bytes());
}

/// A `create_new` temp with an unguessable name (it never follows a planted link), fsync, rename,
/// fsync of the directory; the file keeps the mode `state.json` already had.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::hash::{BuildHasher, Hasher};
    use std::io::Write;
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u32(std::process::id());
    let tmp = path.with_file_name(format!(".state.json.tmp-{:016x}", h.finish()));
    let res = (|| {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        if let Ok(m) = std::fs::metadata(path) {
            let _ = f.set_permissions(m.permissions());
        }
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    match res {
        Ok(()) => {
            if let Some(dir) = path.parent() {
                let _ = cigi_home::platform::sync_dir(dir);
            }
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}
