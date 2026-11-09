// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0

//! The shim (SPEC section 9). Std only; reads one generated text file; never writes.
//! `bin/cig`, `bin/cigo`, ... are copies of this binary. It picks the build a project
//! pins from `installer/shim-index` and execs it. Anything it cannot decide in a few
//! lines goes to the slow path: `ciginstaller __resolve`.
#![forbid(unsafe_code)]

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{exit, Command};
use std::time::Instant;

mod pins;
#[cfg(test)]
mod tests;

use pins::{pin_from_carton_file, pin_from_json, pin_from_toml};

const TOOLS: [&str; 4] = ["cig", "cigo", "cigcompile", "cigar"];
const MAX_DEPTH: usize = 40;
const MAX_FILE: u64 = 64 * 1024;

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

/// The outcome of reading one pin file.
#[derive(Debug, PartialEq)]
enum Reading {
    /// No such file, or not one the shim may trust (a link, another owner):
    /// the same as no file.
    Absent,
    Text(String),
    /// It exists and is ours to read, but not within the cap or not text.
    Unusable,
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

/// Who may own a project pin file: the owner of CIGSCRIPT_HOME, or root. `None` off Unix.
#[derive(Debug, Clone, Copy, Default)]
struct Trusted {
    uid: Option<u32>,
    /// The check is on (Unix); a home whose owner cannot be read then trusts no pin file.
    on: bool,
}

#[cfg(unix)]
fn trusted_for(home: &Path) -> Trusted {
    use std::os::unix::fs::MetadataExt;
    Trusted {
        uid: fs::metadata(home).ok().map(|m| m.uid()),
        on: true,
    }
}
#[cfg(not(unix))]
fn trusted_for(_home: &Path) -> Trusted {
    Trusted::default()
}

#[cfg(unix)]
fn owner_ok(md: &fs::Metadata, t: Trusted) -> bool {
    use std::os::unix::fs::MetadataExt;
    !t.on || md.uid() == 0 || Some(md.uid()) == t.uid
}
#[cfg(not(unix))]
fn owner_ok(_md: &fs::Metadata, _t: Trusted) -> bool {
    true
}

/// Read a pin file: only a regular file (never a link, fifo or device, checked before opening and
/// again on the opened file), owned by a trusted user, at most `MAX_FILE` bytes.
fn read_capped(path: &Path, t: Trusted) -> Reading {
    let Ok(before) = fs::symlink_metadata(path) else {
        return Reading::Absent;
    };
    if !before.file_type().is_file() || !owner_ok(&before, t) {
        return Reading::Absent;
    }
    let Ok(f) = File::open(path) else {
        return Reading::Unusable;
    };
    let Ok(after) = f.metadata() else {
        return Reading::Unusable;
    };
    if !after.is_file() || !owner_ok(&after, t) {
        return Reading::Absent;
    }
    // one byte past the cap tells "exactly at the cap" from "over it"
    let mut bytes = Vec::new();
    if f.take(MAX_FILE + 1).read_to_end(&mut bytes).is_err() || bytes.len() as u64 > MAX_FILE {
        return Reading::Unusable;
    }
    String::from_utf8(bytes).map_or(Reading::Unusable, Reading::Text)
}

/// The index is the installer's own file: read it as before, but never through a link.
fn read_index(path: &Path) -> Option<String> {
    let md = fs::symlink_metadata(path).ok()?;
    if !md.file_type().is_file() {
        return None;
    }
    let mut text = String::new();
    File::open(path)
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

/// Walk up from `start` (at most 40 levels): the first directory with a verdict decides.
///
/// `stop`: the user's home directory. A project inside it is searched up to and including the home
/// and no further, so `/home/.cig-carton` or `/tmp/.cig-carton` never reaches into it.
fn find_pin(start: &Path, legacy: &[&str], t: Trusted, stop: Option<&Path>) -> Pin {
    let stop = stop.filter(|h| start.starts_with(h));
    let mut dir = Some(start);
    for _ in 0..MAX_DEPTH {
        let Some(d) = dir else { break };
        for (file, parse) in [
            (".cig-carton", None),
            (
                "cig-tasks.json",
                Some(pin_from_json as fn(&str) -> Option<String>),
            ),
            (
                "cigar.toml",
                Some(pin_from_toml as fn(&str) -> Option<String>),
            ),
        ] {
            match read_capped(&d.join(file), t) {
                Reading::Absent => {}
                Reading::Unusable => return Pin::Unreadable(d.join(file).display().to_string()),
                Reading::Text(text) => match parse {
                    // a `.cig-carton` is a verdict by itself
                    None => return pin_from_carton_file(&text).map_or(Pin::Slow, Pin::Carton),
                    Some(f) => {
                        if let Some(p) = f(&text) {
                            return Pin::Carton(p);
                        }
                    }
                },
            }
        }
        if legacy
            .iter()
            .any(|n| fs::symlink_metadata(d.join(n)).is_ok_and(|m| m.file_type().is_file()))
        {
            return Pin::Slow;
        }
        if stop == Some(d) {
            break;
        }
        dir = d.parent();
    }
    Pin::None
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
