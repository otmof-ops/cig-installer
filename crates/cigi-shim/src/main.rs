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

mod installer;
#[cfg(test)]
mod tests;

use cigi_shim::{
    choose_home, find, read_script_header, resolve_entry, script_arg, stays_inside, trusted_for,
    Found, HomeChoice, Identity, TargetError, MAX_FILE, TOOLS,
};

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
        /// The index's entry: relative to the home, or (older indexes) absolute inside it.
        path: String,
        /// What the receipt recorded about the file, `dev:ino:size:mtime`; absent in an index
        /// written before the record existed.
        identity: Option<String>,
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
    // `1`: absolute paths; `2`: paths relative to the home, with the receipt's identity record
    if !matches!(index.lines().next(), Some("cigi-index 1" | "cigi-index 2")) {
        return Pick::Slow;
    }
    let rows = || index.lines().map(|l| l.split('\t').collect::<Vec<_>>());
    let find = |kind: &str, key: &str| {
        rows()
            .find(|f| f.len() >= 3 && f[0] == kind && f[1] == key)
            .map(|f| f[2].to_string())
    };
    // (path, identity) of a carton's tool
    let carton_path = |c: &str| {
        rows()
            .find(|f| f.len() >= 4 && f[0] == "carton" && f[1] == c && f[2] == tool)
            .map(|f| (f[3].to_string(), f.get(4).map(|i| i.to_string())))
    };
    let run = |carton: Option<String>, (path, identity): (String, Option<String>)| Pick::Run {
        carton,
        path,
        identity,
    };
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
    rows()
        .find(|f| f.len() >= 3 && f[0] == "stick" && f[1] == tool)
        .map_or(Pick::Slow, |f| {
            run(None, (f[2].to_string(), f.get(3).map(|i| i.to_string())))
        })
}

/// The home this shim works against: `CIGSCRIPT_HOME`, else the home its own file sits in
/// (`<home>/bin/<name>`), else `~/.cigscript`.
fn home_choice(exe: Option<&Path>) -> Option<HomeChoice> {
    let var = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    choose_home(
        exe,
        var("CIGSCRIPT_HOME"),
        var("HOME").or_else(|| var("USERPROFILE")),
    )
}

#[cfg(unix)]
fn launch(mut cmd: Command, argv0: &str) -> std::io::Error {
    use std::os::unix::process::CommandExt;
    cmd.arg0(argv0).exec()
}

/// No exec on Windows: start the program, wait, and exit with its code (the child's own exit
/// status, including an NTSTATUS crash code, is passed through unchanged).
#[cfg(not(unix))]
fn launch(mut cmd: Command, _argv0: &str) -> std::io::Error {
    match cmd.status() {
        Ok(s) => exit(s.code().unwrap_or(1)),
        Err(e) => e,
    }
}

/// E1902: the index leads out of this home (a home that was moved or restored, or an index
/// somebody edited). Nothing is run.
fn outside(name: &str, entry: &str, home: &Path) -> ! {
    die(
        "E1902",
        &format!(
            "the index points `{name}` outside this home ({}): `{entry}`",
            home.display()
        ),
        "ciginstaller repair",
    )
}

/// E1840 when the home the shim chose has no `ciginstaller` in its `bin`. The message names the
/// home that was searched and the shim's own folder, and never says "missing" when a
/// `ciginstaller` sits right beside the shim.
fn no_installer(name: &str, home: &Path, choice: &HomeChoice, exe: Option<&Path>) -> ! {
    let ci = if cfg!(windows) {
        "ciginstaller.exe"
    } else {
        "ciginstaller"
    };
    let dir = exe.and_then(Path::parent);
    let beside = dir.filter(|d| d.join(ci).is_file());
    let shim_dir = dir.map_or(String::new(), |d| {
        format!("; this shim is in {}", d.display())
    });
    match (beside, &choice.anchored) {
        (Some(d), anchored) => {
            let own = anchored.clone().unwrap_or_else(|| d.to_path_buf());
            die(
                "E1840",
                &format!(
                    "nothing installed for `{name}` in {}{shim_dir}, which has its own \
                     ciginstaller (home {})",
                    home.display(),
                    own.display()
                ),
                &format!(
                    "unset CIGSCRIPT_HOME, or set CIGSCRIPT_HOME={}",
                    own.display()
                ),
            )
        }
        (None, _) => die(
            "E1840",
            &format!(
                "nothing installed for `{name}` in {}{shim_dir}, and there is no ciginstaller \
                 in {}",
                home.display(),
                home.join("bin").display()
            ),
            "reinstall ciginstaller from https://github.com/otmof-ops/cig-installer",
        ),
    }
}

fn main() {
    let t0 = Instant::now();
    let mut args = std::env::args_os();
    let argv0 = args.next().unwrap_or_default();
    // Windows has no argv[0] to trust (`cig` is a fine way to start `cig.exe`): the name is the
    // running file's own, compared without regard to case.
    let own = if cfg!(windows) {
        std::env::current_exe().unwrap_or_else(|_| PathBuf::from(&argv0))
    } else {
        PathBuf::from(&argv0)
    };
    let name = installer::tool_name(&own, cfg!(windows));
    // Ctrl+C belongs to the program this one waits for
    #[cfg(windows)]
    cigi_sys::process::swallow_ctrl_c();
    let is_installer = cfg!(windows) && name == "cigi";
    if !is_installer && !TOOLS.contains(&name.as_str()) {
        die(
            "E1844",
            &format!("this file is named `{name}`, which is not a tool the installer shims"),
            "ciginstaller shims sync",
        );
    }
    let exe = std::env::current_exe().ok();
    let Some(choice) = home_choice(exe.as_deref()) else {
        die(
            "E1835",
            "cannot find the home directory",
            "set CIGSCRIPT_HOME to a directory you own",
        );
    };
    let home = choice.home.clone();
    if std::env::var_os("CIGI_SHIM_VERBOSE").is_some_and(|v| v == "1") {
        match choice.disagreement() {
            Some((env, own)) => eprintln!(
                "note: CIGSCRIPT_HOME is {} but this shim is in {}, which belongs to {}; \
                 CIGSCRIPT_HOME wins",
                env.display(),
                own.join("bin").display(),
                own.display()
            ),
            None => eprintln!("note: home is {}", home.display()),
        }
    }
    let rest: Vec<OsString> = args.collect();
    if is_installer {
        match installer::run(&home, &rest) {
            Ok(code) => exit(code),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => die(
                "E1840",
                &format!(
                    "cannot start ciginstaller from {}: nothing is there",
                    home.join("bin").display()
                ),
                "reinstall ciginstaller from https://github.com/otmof-ops/cig-installer",
            ),
            Err(e) => die(
                "E1831",
                &format!("cannot start ciginstaller: {e}"),
                "ciginstaller repair",
            ),
        }
    }
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
    // A script that pins its cig (`# cig >=1.6.2`) is the resolver's to place, so the build that
    // runs is one that satisfies the header. Only `cig` reads headers; only the one argument that
    // names the script is looked at, and only when it is a readable file; `CIGI_CARTON` is the
    // user's own word and wins, as it does everywhere.
    let has_header = matches!(pin, Pin::None | Pin::Carton(_))
        && name == "cig"
        && forced.is_none()
        && script_arg(&rest).is_some_and(|a| read_script_header(Path::new(a)).is_some());
    let pin = if has_header { Pin::Slow } else { pin };
    if let Pin::Unreadable(file) = &pin {
        eprintln!(
            "note: {file} is too large or not text for the shim; \
             the installer will read it (slower)"
        );
    }
    // A tool runs under its own name; the installer, on the slow path, under its own, so its
    // misnamed-binary guard (E1844) holds.
    // A carton the machine locks outranks everything the fast path reads (#17): when the machine
    // layer mentions `carton`, the installer's resolver applies it.
    let picked = if cigi_shim::machine::machine_layer_mentions_carton() {
        Pick::Slow
    } else {
        pick(&index, &name, forced.as_deref(), &pin)
    };
    let (mut cmd, carton, run_as) = match picked {
        Pick::Run {
            carton,
            path,
            identity,
        } => {
            let target = match resolve_entry(&home, &path) {
                Ok(t) => t,
                Err(TargetError::NotPlain) => die(
                    "E1844",
                    &format!("the index gives `{name}` a path that is not plain: `{path}`"),
                    "ciginstaller shims sync",
                ),
                Err(TargetError::Outside) => outside(&name, &path, &home),
            };
            if target.starts_with(home.join("bin")) {
                die(
                    "E1844",
                    &format!("the index points `{name}` at the shim itself"),
                    "ciginstaller shims sync",
                );
            }
            // Resolve the target once, links followed, and run what that resolved to: the check
            // and the exec then name the same file.
            let target = match stays_inside(&home, &target) {
                Some(t) => t,
                None if target.symlink_metadata().is_ok() => outside(&name, &path, &home),
                None => die(
                    "E1840",
                    &format!(
                        "the build for `{name}` is missing on disk: {}",
                        target.display()
                    ),
                    "ciginstaller repair",
                ),
            };
            if let Some(want) = identity.as_deref().and_then(Identity::parse) {
                // one stat, no hashing: the receipt's hash is `ciginstaller repair`'s to check
                if Identity::measure(&target).map_or(true, |got| got != want) {
                    die(
                        "E1903",
                        &format!(
                            "the build for `{name}` at {} is not the file its receipt recorded \
                             (a moved or restored home, or the file was replaced)",
                            target.display()
                        ),
                        "ciginstaller repair",
                    );
                }
            }
            let mut c = Command::new(target);
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
                no_installer(&name, &home, &choice, exe.as_deref());
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
    // A home found from where the shim sits is passed on, so the installer's slow path and the
    // tool (cigo looking for cig) work against the same home without anyone exporting it.
    if choice.env.is_none() && choice.anchored.is_some() {
        cmd.env("CIGSCRIPT_HOME", &home);
    }
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
