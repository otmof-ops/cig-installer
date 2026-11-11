// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `shims sync` and `shims list`.
//!
//! `sync` makes `bin/` and `installer/shim-index` say what `state.json` and the receipts say,
//! in one transaction and without asking whether anything changed: a deleted shim is linked
//! again, a stray file under a family name is replaced, one left over from a tool that is no
//! longer installed goes, and a hand-edited index is written afresh.

use cigi_home::shims::{shim_path, shims_present};
use cigi_home::{platform, remove_shims, sync_shims, ShimIndex};
use cigi_ui::{Align, Cell, InstallerError, Report, Table};
use serde_json::json;

use super::tabular::details;
use super::Outcome;
use crate::cli::ShimsSub;
use crate::cx::Cx;
use crate::errors::ie;
use crate::flows::switch::{shim_names, shim_object};
use crate::flows::{cached_yanked, save_and_flip, settle};
use crate::resolve::{resolve, Mode};

pub fn run(cx: &mut Cx, sub: &ShimsSub) -> Result<Outcome, InstallerError> {
    match sub {
        ShimsSub::Sync => sync(cx),
        ShimsSub::List => list(cx),
    }
}

fn nothing(why: &str, fix: &str) -> InstallerError {
    InstallerError::new("E1874", why).with_fix(fix)
}

fn sync(cx: &mut Cx) -> Result<Outcome, InstallerError> {
    cx.lock_and_recover()?;
    let state = cx.state.clone();
    if state.cartons.is_empty() && state.sticks.is_empty() {
        return Err(nothing(
            "nothing is installed, so there are no shims to build",
            "ciginstaller install carton",
        ));
    }
    let names: Vec<String> = shim_names(&cx.home, &state).into_iter().collect();
    let obj = shim_object(&cx.home, &state).ok_or_else(|| {
        nothing(
            "the installer's own toolchain has no shim program to link the shims to",
            "ciginstaller repair",
        )
    })?;
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let fine = shims_present(&cx.home, &obj, &refs);
    let redone: Vec<String> = names
        .iter()
        .filter(|n| !fine.contains(n))
        .cloned()
        .collect();
    // family shims in bin/ that no installed tool serves any more
    let stale: Vec<String> = cigi_shim::SHIMS
        .iter()
        .map(|(n, _)| *n)
        .filter(|s| !names.iter().any(|n| n == s) && shim_path(&cx.home, s).exists())
        .map(|s| s.to_string())
        .collect();
    let index_before = std::fs::read_to_string(cx.home.shim_index_path()).ok();
    let yanked = cached_yanked(cx);
    let mut txn = cx.begin("shims sync")?;
    let done = (|| {
        sync_shims(&mut txn, &cx.home, &refs, &obj).map_err(ie)?;
        let gone: Vec<&str> = stale.iter().map(String::as_str).collect();
        remove_shims(&mut txn, &cx.home, &gone).map_err(ie)?;
        save_and_flip(&mut txn, &cx.home, &state, yanked.clone())
    })();
    settle(txn, done)?;
    let index_after = std::fs::read_to_string(cx.home.shim_index_path()).ok();
    let index_rewritten = index_before != index_after;
    let mut r = Report::done(format!("{} shim(s) match what is installed", names.len()));
    if !redone.is_empty() {
        r = r.detail(format!("made again: {}", redone.join(", ")));
    }
    if !stale.is_empty() {
        r = r.detail(format!(
            "removed, nothing installed serves them: {}",
            stale.join(", ")
        ));
    }
    if index_rewritten {
        r = r.detail("the shim index was written again from state");
    }
    if redone.is_empty() && stale.is_empty() && !index_rewritten {
        r = r.detail("nothing had drifted");
    }
    Ok(r.data(json!({
        "shims": names,
        "relinked": redone,
        "removed": stale,
        "index_rewritten": index_rewritten,
    }))
    .into())
}

/// Is the first `name` on PATH the one in this home's `bin/`?
fn first_on_path(cx: &Cx, name: &str) -> bool {
    let canon = |p: &std::path::Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let bin = canon(&cx.home.bin());
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    let file = format!("{name}{}", platform::exe_suffix());
    std::env::split_paths(&path)
        .find(|dir| dir.join(&file).is_file())
        .is_some_and(|dir| canon(&dir) == bin)
}

fn list(cx: &mut Cx) -> Result<Outcome, InstallerError> {
    let names: Vec<String> = shim_names(&cx.home, &cx.state).into_iter().collect();
    if names.is_empty() {
        return Ok(Report::done("no shims: nothing is installed")
            .next("ciginstaller install carton")
            .data(json!({ "shims": [] }))
            .into());
    }
    let mut table = Table::new(&[
        ("shim", Align::Left),
        ("runs here", Align::Left),
        ("on PATH", Align::Left),
    ]);
    let mut shims = Vec::new();
    let mut missing = 0;
    for n in &names {
        let present = shim_path(&cx.home, n).is_file();
        if !present {
            missing += 1;
        }
        let target = resolve(cx, n, Mode::Which)
            .map(|res| res.path.display().to_string())
            .map_err(|e| e.message);
        let first = present && first_on_path(cx, n);
        table = table.row(vec![
            Cell::new(n.clone()),
            match &target {
                Ok(p) => Cell::new(p.clone()),
                Err(m) => Cell::changed(format!("nothing: {m}")),
            },
            if !present {
                Cell::changed("missing")
            } else if first {
                Cell::new("first")
            } else {
                Cell::new("not first")
            },
        ]);
        shims.push(json!({
            "name": n,
            "target": target.as_ref().ok(),
            "error": target.as_ref().err(),
            "present": present,
            "first_on_path": first,
        }));
    }
    let mut r = Report::done(format!("{} shim(s)", names.len()));
    for l in details(&cx.ui, &table) {
        r = r.detail(l);
    }
    if missing > 0 {
        r = r.detail(format!("{missing} shim file(s) are missing from bin/"));
        r = r.next("ciginstaller shims sync");
    }
    // the index is part of the picture: a hand edit shows here before it is repaired
    let drift = match ShimIndex::load(&cx.home) {
        Ok(Some(ix)) => {
            ix.render()
                != ShimIndex::from_state(&cx.home, &cx.state)
                    .with_yanked(cached_yanked(cx))
                    .render()
        }
        _ => true,
    };
    if drift {
        r = r.detail("the shim index differs from what state says");
        r = r.next("ciginstaller shims sync");
    }
    Ok(
        r.data(json!({ "shims": shims, "index_matches_state": !drift }))
            .into(),
    )
}
