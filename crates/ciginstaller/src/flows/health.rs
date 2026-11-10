// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! What `doctor`, `repair` and `uninstall` share: which shims the state calls for, which store
//! objects the installer's own links point at, what the user chose about PATH.

use std::collections::BTreeSet;
use std::path::Path;

use cig_compat::Version;
use cigi_home::{installed, read_receipt, Home, ObjectId, Shell, State};

/// Every `(tool, version)` the state names: the cartons' tools and the sticks.
pub fn builds(state: &State) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for r in state.cartons.values() {
        out.extend(r.tools.iter().map(|(t, v)| (t.clone(), v.clone())));
    }
    out.extend(
        state
            .sticks
            .iter()
            .map(|s| (s.tool.clone(), s.version.clone())),
    );
    out.sort();
    out.dedup();
    out
}

/// The shim names the state calls for: the names the shim binary serves, from the receipts of the
/// builds the state lists (the same rule `install` follows).
pub fn shim_names(home: &Home, state: &State) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for (t, v) in builds(state) {
        if let Some(r) = Version::parse(&v)
            .ok()
            .and_then(|v| read_receipt(home, &t, &v).ok())
        {
            names.extend(
                r.shims
                    .iter()
                    .map(|s| s.name.clone())
                    .filter(|n| cigi_shim::TOOLS.contains(&n.as_str())),
            );
        }
    }
    names
}

/// The store objects of the installer's own toolchain: `(ciginstaller, cigi-shim)`. The version
/// the state names first, then this build's, then any installed.
pub fn installer_objects(home: &Home, state: &State) -> Option<(ObjectId, ObjectId)> {
    let mut versions: Vec<Version> = Vec::new();
    for v in [
        Some(state.installer.version.clone()),
        Some(env!("CARGO_PKG_VERSION").to_string()),
    ]
    .into_iter()
    .flatten()
    {
        if let Ok(v) = Version::parse(&v) {
            versions.push(v);
        }
    }
    versions.extend(
        installed(home)
            .into_iter()
            .filter(|(t, _)| t == "ciginstaller")
            .map(|(_, v)| v)
            .rev(),
    );
    for v in versions {
        let Ok(r) = read_receipt(home, "ciginstaller", &v) else {
            continue;
        };
        let id = |path: &str| {
            r.files
                .iter()
                .find(|f| f.path == path)
                .map(|f| ObjectId::new(f.sha256.clone(), f.exec))
        };
        if let (Some(a), Some(b)) = (id("bin/ciginstaller"), id("bin/cigi-shim")) {
            return Some((a, b));
        }
    }
    None
}

/// The shell `install` added a PATH block for, when `state.json` says it did.
pub fn recorded_shell(state: &State) -> Option<Shell> {
    state
        .extra
        .get("path_block")?
        .get("shell")?
        .as_str()
        .and_then(Shell::from_program)
}

pub fn size_text(n: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut x = n as f64;
    let mut u = 0;
    while x >= 1024.0 && u < UNITS.len() - 1 {
        x /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{x:.1} {}", UNITS[u])
    }
}

/// After a commit, the saved copies a transaction kept for its own rollback hold bytes nothing
/// needs (damaged files, removed toolchains): drop the directory.
pub fn remove_committed_txn(home: &Home, id: &str) {
    let dir = home.txn_root().join(id);
    let plain = !id.is_empty()
        && !id.contains(['/', '\\'])
        && id != "."
        && id != ".."
        && Path::new(id).is_relative();
    if plain && dir.starts_with(home.txn_root()) {
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Shim names (including the installer's own `ciginstaller` and `cigi`) that are missing from
/// `bin/` or are not links of the object they must be. `None` when the installer's own toolchain
/// has no readable receipt, so nobody can tell what the shims should be.
pub fn missing_shims(home: &Home, state: &State) -> Option<Vec<String>> {
    let (exe, shim) = installer_objects(home, state)?;
    let names = shim_names(home, state);
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let present: BTreeSet<String> = cigi_home::shims::shims_present(home, &shim, &refs)
        .into_iter()
        .collect();
    let own: BTreeSet<String> =
        cigi_home::shims::shims_present(home, &exe, &["ciginstaller", "cigi"])
            .into_iter()
            .collect();
    let mut missing: Vec<String> = names.difference(&present).cloned().collect();
    missing.extend(
        ["ciginstaller", "cigi"]
            .iter()
            .filter(|n| !own.contains(**n))
            .map(|n| n.to_string()),
    );
    Some(missing)
}

/// How `installer/shim-index` stands against what `state.json` produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexState {
    Agrees,
    Differs,
    Missing,
}

/// `yanked`: the cartons the cached manifest yanks (the index leaves a yanked carton out of a
/// loose pin). A file that matches the render with or without them counts as agreeing, because a
/// flow may have held a fresher manifest than the cache.
pub fn index_state(home: &Home, state: &State, yanked: Vec<String>) -> IndexState {
    let with = cigi_home::ShimIndex::from_state(home, state)
        .with_yanked(yanked)
        .render();
    let without = cigi_home::ShimIndex::from_state(home, state).render();
    match std::fs::read_to_string(home.shim_index_path()) {
        Ok(h) if h == with || h == without => IndexState::Agrees,
        Ok(_) => IndexState::Differs,
        Err(_) => IndexState::Missing,
    }
}

/// The cig versions whose humidor mirror the receipt promised and the home does not hold:
/// `(version, entry is there, link is the installed binary)`.
pub fn humidor_bad(home: &Home) -> Vec<(Version, bool, bool)> {
    use cigi_home::humidor::{self, SOURCE};
    use cigi_home::platform;
    let entries = humidor::entries(home);
    let mut out = Vec::new();
    for (t, ver) in installed(home) {
        if t != "cig" {
            continue;
        }
        let Ok(r) = read_receipt(home, &t, &ver) else {
            continue;
        };
        if r.humidor_mirror.as_deref() != Some("mirrored") {
            continue;
        }
        let link = home
            .humidor()
            .join(ver.to_string())
            .join(format!("cig{}", platform::exe_suffix()));
        let entry_ok = entries.as_ref().is_ok_and(|es| {
            es.iter()
                .any(|e| e.version == ver.to_string() && e.source == SOURCE)
        });
        let link_ok = match (
            platform::ino(&link),
            super::tool_exe(home, &t, &ver).and_then(|p| platform::ino(&p)),
        ) {
            (Some(a), Some(b)) => a == b,
            (None, _) => false,
            _ => link.is_file(),
        };
        if !entry_ok || !link_ok {
            out.push((ver, entry_ok, link_ok));
        }
    }
    out
}
