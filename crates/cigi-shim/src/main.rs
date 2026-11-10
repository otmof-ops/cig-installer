// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! The shim (SPEC section 9). Std only; reads one generated text file; never writes.
//! `bin/cig`, `bin/cigo`, ... are copies of this binary. It picks the build a project
//! pins from `installer/shim-index` and execs it. Anything it cannot decide in a few
//! lines goes to the slow path: `ciginstaller __resolve`.
#![forbid(unsafe_code)]

use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{exit, Command};
use std::time::Instant;

#[cfg(test)]
mod tests;

use cigi_shim::{find, trusted_for, Found, MAX_FILE, TOOLS};

/// What the walk up from the working directory found.
#[derive(Debug, PartialEq)]
enum Pin {
    Carton(String),
    Slow,
    /// A pin file is there but cannot be read here (over the cap, not UTF-8,
    /// read error): the slow path decides, and says so. The text names the file.
    Unreadable(String),
    None,
}

/// What the index says to run.
#[derive(Debug, PartialEq)]
enum Pick {
    Run {
        carton: Option<String>,
        path: String,
    },
    Slow,
}

fn die(code: &str, msg: &str, fix: &str) -> ! {
    eprintln!(
        "error[{code} installer]: {msg}\n  = explain: ciginstaller explain {code}\n  = fix: {fix}"
    );
    exit(127)
}

/// The index is the installer's own file: read it as before, but never through a link.
fn read_index(path: &Path) -> Option<String> {
    let md = fs::symlink_metadata(path).ok()?;
    if !md.file_type().is_file() {
        return None;
    }
    let mut text = String::new();
    cigi_shim::open_nonblocking(path)
        .ok()?
        .take(MAX_FILE)
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}

fn legacy_names(index: &str) -> Vec<&str> {
    let mut names = Vec::new();
    for line in index.lines() {
        let mut f = line.split('\t');
        if f.next() == Some("legacy-pin-files") {
            names.extend(f.filter(|n| !n.is_empty() && !n.contains('/')));
        }
    }
    names
}

/// Walk up from `start` with the shared reader (`cigi_shim::find`) and say what the shim does
/// with what it found.
fn find_pin(start: &Path, legacy: &[&str], t: cigi_shim::Trusted, stop: Option<&Path>) -> Pin {
    match find(start, legacy, t, stop, MAX_FILE) {
        // a `.cig-carton` is a verdict by itself: one with no pin in it is the resolver's to explain
        Found::Pin { pin: None, .. } | Found::Legacy { .. } => Pin::Slow,
        Found::Pin { pin: Some(p), .. } => Pin::Carton(p),
        Found::Unusable { file, .. } => Pin::Unreadable(file.display().to_string()),
        Found::Nothing => Pin::None,
    }
}

/// Look the pin up in the index. `>=` and anything unusual goes to the slow path.
fn pick(index: &str, tool: &str, forced: Option<&str>, pin: &Pin) -> Pick {
    if index.lines().next() != Some("cigi-index 1") {
        return Pick::Slow;
    }
    let rows = || index.lines().map(|l| l.split('\t').collect::<Vec<_>>());
    let find = |kind: &str, key: &str| {
        rows()
            .find(|f| f.len() >= 3 && f[0] == kind && f[1] == key)
            .map(|f| f[2].to_string())
    };
    let carton_path = |c: &str| {
        rows()
            .find(|f| f.len() >= 4 && f[0] == "carton" && f[1] == c && f[2] == tool)
            .map(|f| f[3].to_string())
    };
    let run = |carton: Option<String>, path: String| Pick::Run { carton, path };
    let chosen = match (forced, pin) {
        (Some(c), _) => Some(c.to_string()),
        (None, Pin::Slow | Pin::Unreadable(_)) => return Pick::Slow,
        (None, Pin::Carton(p)) => {
            // cig-compat's `Pin::parse` grammar: leading `v`s, then digits and
            // dots only (so no prerelease form). Anything else is the
            // resolver's to answer.
            let p = p.trim_start_matches('v');
            if p.is_empty()
                || p.split('.')
                    .any(|x| x.is_empty() || !x.bytes().all(|b| b.is_ascii_digit()))
            {
                return Pick::Slow;
            }
            match p.matches('.').count() {
                0 => Some(find("carton-major", p).unwrap_or_default()),
                1 => Some(find("carton-minor", p).unwrap_or_default()),
                _ => Some(p.to_string()),
            }
        }
        (None, Pin::None) => None,
    };
    if let Some(c) = chosen {
        // an empty name (a pin the index does not know) matches no carton line: slow path
        return carton_path(&c).map_or(Pick::Slow, |path| run(Some(c), path));
    }
    if let Some(c) = rows()
        .find(|f| f.len() >= 2 && f[0] == "machine")
        .map(|f| f[1].to_string())
    {
        if let Some(path) = carton_path(&c) {
            return run(Some(c), path);
        }
    }
    find("stick", tool).map_or(Pick::Slow, |path| run(None, path))
}

fn home() -> Option<PathBuf> {
    let var = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    var("CIGSCRIPT_HOME").or_else(|| {
        var("HOME")
            .or_else(|| var("USERPROFILE"))
            .map(|h| h.join(".cigscript"))
    })
}

#[cfg(unix)]
fn launch(mut cmd: Command, argv0: &str) -> std::io::Error {
    use std::os::unix::process::CommandExt;
    cmd.arg0(argv0).exec()
}

#[cfg(not(unix))]
fn launch(mut cmd: Command, _argv0: &str) -> std::io::Error {
    match cmd.status() {
        Ok(s) => exit(s.code().unwrap_or(1)),
        Err(e) => e,
    }
}

fn main() {
    let t0 = Instant::now();
    let mut args = std::env::args_os();
    let argv0 = args.next().unwrap_or_default();
    let name = Path::new(&argv0)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    if !TOOLS.contains(&name.as_str()) {
        die(
            "E1844",
            &format!("this file is named `{name}`, which is not a tool the installer shims"),
            "ciginstaller shims sync",
        );
    }
    let Some(home) = home() else {
        die(
            "E1835",
            "cannot find the home directory",
            "set CIGSCRIPT_HOME to a directory you own",
        );
    };
    let rest: Vec<OsString> = args.collect();
    let forced = std::env::var("CIGI_CARTON").ok().filter(|s| !s.is_empty());
    let index = read_index(&home.join("installer").join("shim-index")).unwrap_or_default();
    let user_home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    let pin = match (&forced, std::env::current_dir()) {
        (None, Ok(cwd)) => find_pin(
            &cwd,
            &legacy_names(&index),
            trusted_for(&home),
            user_home.as_deref(),
        ),
        _ => Pin::None,
    };
    if let Pin::Unreadable(file) = &pin {
        eprintln!(
            "note: {file} is too large or not text for the shim; \
             the installer will read it (slower)"
        );
    }
    // A tool runs under its own name; the installer, on the slow path, under its own, so its
    // misnamed-binary guard (E1844) holds.
    let (mut cmd, carton, run_as) = match pick(&index, &name, forced.as_deref(), &pin) {
        Pick::Run { carton, path } => {
            let target = Path::new(&path);
            if !target.is_absolute()
                || target
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                die(
                    "E1844",
                    &format!("the index gives `{name}` a path that is not absolute and plain"),
                    "ciginstaller shims sync",
                );
            }
            if target.starts_with(home.join("bin")) {
                die(
                    "E1844",
                    &format!("the index points `{name}` at the shim itself"),
                    "ciginstaller shims sync",
                );
            }
            let mut c = Command::new(path);
            c.args(&rest);
            (c, carton, name.as_str())
        }
        Pick::Slow => {
            let installer = home.join("bin").join(if cfg!(windows) {
                "ciginstaller.exe"
            } else {
                "ciginstaller"
            });
            if !installer.is_file() {
                die(
                    "E1840",
                    &format!("nothing installed for `{name}`, and ciginstaller is missing"),
                    "reinstall ciginstaller from https://github.com/otmof-ops/cig-installer",
                );
            }
            let cwd = std::env::current_dir().unwrap_or_default();
            let mut c = Command::new(installer);
            c.args(["__resolve", "--tool", &name, "--cwd"])
                .arg(cwd)
                .arg("--")
                .args(&rest);
            (c, None, "ciginstaller")
        }
    };
    cmd.env("CIGI_MANAGED", "1");
    if let Some(c) = &carton {
        cmd.env("CIGI_CARTON", c);
    }
    if std::env::var_os("CIGI_SHIM_TIMING").is_some_and(|v| v == "1") {
        eprintln!("cigi-shim-timing-ns: {}", t0.elapsed().as_nanos());
    }
    let err = launch(cmd, run_as);
    if err.kind() == std::io::ErrorKind::NotFound {
        die(
            "E1840",
            &format!("the build for `{name}` is missing on disk"),
            "ciginstaller repair",
        );
    }
    die(
        "E1831",
        &format!("cannot start `{name}`: {err}"),
        "ciginstaller repair",
    )
}
