// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! The installer's side of the family contract (docs/decisions/0003): what it does for cig, cigo,
//! cigcompile and cigar beyond installing them.
//!
//! - `cigo guard uninstall --yes`, run before cigo is removed, when the guard is installed;
//! - `cig crash announce-fixed`, run after an update that changed cig;
//! - the product caches `uninstall` clears: cigar's run cache and cigcompile's fetched runtimes.
//!
//! None of this touches user data. A cache is rebuilt by the product that owns it.

use std::path::{Path, PathBuf};

use cig_compat::Version;
use cigi_home::{installed, Home, State, Txn};
use cigi_ui::{InstallerError, EXIT_FAILED};

use super::{doctor, tool_exe};

/// The version of `tool` this machine runs: the stick, else the machine carton's build.
pub fn installed_version(st: &State, tool: &str) -> Option<Version> {
    st.sticks
        .iter()
        .find(|s| s.tool == tool)
        .map(|s| s.version.clone())
        .or_else(|| {
            st.machine_carton
                .as_ref()
                .and_then(|c| st.cartons.get(c))
                .and_then(|r| r.tools.get(tool).cloned())
        })
        .and_then(|v| Version::parse(&v).ok())
}

// ------------------------------------------------------------------ the cigo guard

/// cigo's own state directory: `CIGO_HOME`, else `.cig-orchestrator` in the user's home.
fn cigo_home(home: &Home) -> PathBuf {
    std::env::var_os("CIGO_HOME")
        .filter(|v| !v.is_empty())
        .map_or_else(|| home.user_home().join(".cig-orchestrator"), PathBuf::from)
}

/// Is cigo's guard installed? Its state file, or any shim in its `shims/` directory, says so
/// (orchestrate.cig: `GUARD_STATE_FILE` and `SHIMS_DIR` under `CIGO_HOME`).
pub fn guard_installed(home: &Home) -> bool {
    let base = cigo_home(home);
    base.join("guard").join("state.json").is_file()
        || std::fs::read_dir(base.join("shims")).is_ok_and(|mut r| r.next().is_some())
}

/// The cigo to run: the build of it installed here, when there is one.
fn cigo_build(home: &Home, st: &State) -> Option<Version> {
    installed_version(st, "cigo").or_else(|| {
        installed(home)
            .into_iter()
            .filter(|(t, _)| t == "cigo")
            .map(|(_, v)| v)
            .max()
    })
}

/// Does removing cigo have a guard to take down first? (cigo is installed and so is the guard.)
pub fn guard_pending(home: &Home, st: &State) -> bool {
    cigo_build(home, st).is_some() && guard_installed(home)
}

fn stopped(what: String, fix: &str) -> InstallerError {
    InstallerError::new("E1829", what)
        .with_hint("cigo's guard shims and PATH blocks are still in place, and nothing was removed")
        .with_fix(fix)
        .with_exit(EXIT_FAILED)
}

/// Run `cigo guard uninstall --yes` before cigo goes, when the guard is installed. `Ok(None)` is
/// "no guard"; `Ok(Some(line))` says it was taken down. Any failure stops the uninstall (E1829):
/// removing cigo with the guard still on `PATH` would leave shims that call a program that is not
/// there.
pub fn guard_uninstall(home: &Home, st: &State) -> Result<Option<String>, InstallerError> {
    if !guard_pending(home, st) {
        return Ok(None);
    }
    let by_hand = "cigo guard uninstall --yes";
    let Some(ver) = cigo_build(home, st) else {
        return Ok(None);
    };
    let Some(exe) = tool_exe(home, "cigo", &ver) else {
        return Err(stopped(
            format!("cigo {ver} cannot be run to take its guard down: it has no readable receipt"),
            "ciginstaller repair",
        ));
    };
    if !cigi_home::verify_toolchain(home, "cigo", &ver).is_empty() {
        return Err(stopped(
            format!("cigo {ver} cannot be run to take its guard down: its files differ from its receipt"),
            "ciginstaller repair",
        ));
    }
    let bin = home.bin();
    match doctor::run_args(
        &exe,
        &["guard", "uninstall", "--yes"],
        &home.root(),
        120,
        Some(&bin),
    ) {
        Ok(r) if r.code == Some(0) => Ok(Some(
            "ran `cigo guard uninstall --yes` first: the guard's shims and PATH blocks are gone"
                .to_string(),
        )),
        Ok(r) => {
            let said = tail_of(&format!("{}{}", r.stdout, r.stderr));
            Err(stopped(
                format!(
                    "`cigo guard uninstall --yes` failed ({}): {said}",
                    r.code
                        .map_or("ended by a signal".to_string(), |c| format!("exit {c}"))
                ),
                by_hand,
            ))
        }
        Err(why) => Err(stopped(
            format!("`cigo guard uninstall --yes` could not be run: {why}"),
            by_hand,
        )),
    }
}

/// The last few lines of what a tool said, on one line, for an error.
fn tail_of(text: &str) -> String {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let from = lines.len().saturating_sub(3);
    let joined = lines[from..].join(" / ");
    if joined.is_empty() {
        "it said nothing".to_string()
    } else {
        joined.chars().take(400).collect()
    }
}

// ------------------------------------------------------------------ announce-fixed

/// After an update that changed cig: `cig crash announce-fixed`, from the new cig, so a bug this
/// machine reported that the new release fixed is said once. Returns the lines to show. Never an
/// error: a cig without the subcommand (a usage exit) is skipped without a word, and anything
/// else that goes wrong becomes one line.
pub fn announce_fixed(home: &Home, cig: &Version) -> Vec<String> {
    let Some(exe) = tool_exe(home, "cig", cig) else {
        return Vec::new();
    };
    match doctor::run_args(&exe, &["crash", "announce-fixed"], &home.root(), 30, None) {
        Ok(r) if r.code == Some(0) => r
            .stdout
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect(),
        // clap's usage error (2) or cig's own (3): this build lacks the subcommand
        Ok(r) if matches!(r.code, Some(2 | 3)) => Vec::new(),
        Ok(r) => vec![format!(
            "`cig crash announce-fixed` did not finish well ({}): {}",
            r.code
                .map_or("ended by a signal".to_string(), |c| format!("exit {c}")),
            tail_of(&r.stderr)
        )],
        Err(why) => vec![format!(
            "`cig crash announce-fixed` could not be run: {why}"
        )],
    }
}

// ------------------------------------------------------------------ product caches

/// A cache folder a product keeps, and what it is.
#[derive(Debug, Clone)]
pub struct CacheDir {
    pub what: &'static str,
    pub path: PathBuf,
}

/// cigar's run cache: `<cache base>/cigar/run`, the base being `XDG_CACHE_HOME`, else
/// `LOCALAPPDATA` on Windows or `~/.cache` elsewhere (src/run.rs `cache_dir` in cigar-script).
fn cigar_run_cache(home: &Home) -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            if cfg!(windows) {
                std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
            } else {
                Some(home.user_home().join(".cache"))
            }
        })
        .unwrap_or_else(std::env::temp_dir);
    base.join("cigar").join("run")
}

/// The cache folders of `tool` that exist now. cigcompile keeps fetched runtimes in
/// `<home>/cigcompile/runtimes` and its compile index in `<home>/cache/compile` (src/fetch.rs and
/// src/cache.rs in cig-compile). Other tools keep none.
pub fn caches_of(home: &Home, tool: &str) -> Vec<CacheDir> {
    let root = home.root();
    let all = match tool {
        "cigar" => vec![CacheDir {
            what: "cigar's run cache",
            path: cigar_run_cache(home),
        }],
        "cigcompile" => vec![
            CacheDir {
                what: "cigcompile's fetched runtimes",
                path: root.join("cigcompile").join("runtimes"),
            },
            CacheDir {
                what: "cigcompile's compile cache",
                path: root.join("cache").join("compile"),
            },
        ],
        _ => Vec::new(),
    };
    all.into_iter()
        .filter(|c| std::fs::symlink_metadata(&c.path).is_ok())
        .collect()
}

/// The caches of every product, for `uninstall --all`.
pub fn all_caches(home: &Home) -> Vec<CacheDir> {
    let mut v = caches_of(home, "cigar");
    v.extend(caches_of(home, "cigcompile"));
    v
}

/// Journal the removal of each cache. A cache the transaction cannot take (outside the home and
/// the user's home, or behind a link) is left where it is and said so: it never stops the
/// uninstall. Returns the paths removed.
pub fn delete_caches(
    txn: &mut Txn,
    home: &Home,
    dirs: &[CacheDir],
    notes: &mut Vec<String>,
) -> Vec<String> {
    let mut removed = Vec::new();
    for c in dirs {
        let inside_home = cigi_home::platform::starts_with(&c.path, &home.root());
        let inside_user = cigi_home::platform::starts_with(&c.path, &home.user_home());
        let res = if inside_home {
            txn.delete(&c.path)
        } else if inside_user {
            txn.delete_user_file(&c.path)
        } else {
            notes.push(format!(
                "{} is outside your home, so it is left where it is: {}",
                c.what,
                c.path.display()
            ));
            continue;
        };
        match res {
            Ok(()) => removed.push(format!("{} ({})", c.path.display(), c.what)),
            Err(e) => notes.push(format!(
                "{} was left in place ({}): {e}",
                c.what,
                c.path.display()
            )),
        }
    }
    removed
}

/// After the commit: take away the parent folders the caches leave empty. Nothing else is
/// touched; `remove_dir` refuses a folder with anything in it.
pub fn tidy_parents(home: &Home, dirs: &[CacheDir]) {
    let root = home.root();
    for c in dirs {
        if let Some(p) = c.path.parent() {
            if p != Path::new("") && !cigi_home::platform::same_path(p, &root) {
                let _ = std::fs::remove_dir(p);
            }
        }
    }
}

/// Is `entry` of the home only a folder of caches (so `uninstall --all` does not list it as
/// something that stays)? `cache/` holds only `compile`, `cigcompile/` only `runtimes`.
pub fn only_caches(home: &Home, entry: &str) -> bool {
    let keep = match entry {
        "cache" => "compile",
        "cigcompile" => "runtimes",
        _ => return false,
    };
    std::fs::read_dir(home.root().join(entry)).is_ok_and(|rd| {
        rd.flatten()
            .all(|e| e.file_name().to_string_lossy() == keep)
    })
}
