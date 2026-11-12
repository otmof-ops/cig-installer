// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! The one reader of project pin files (SPEC sections 6.5 and 8), shared by the shim and by
//! `ciginstaller`, so the fast path and the resolver cannot disagree about which file counts.
//! Std only, no dependencies (decision D6). It never writes.
//!
//! The rules, for every pin file (the three current ones and the legacy per-tool ones):
//! only a regular file (never a link, fifo, socket or device: checked before opening, and again
//! on the opened file), owned by the owner of `CIGSCRIPT_HOME` or by root. Anything else is the
//! same as no file. The walk climbs at most 40 levels and stops at `$HOME` when it starts inside it.
#![forbid(unsafe_code)]

use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

pub mod machine;
pub mod pins;

pub use pins::ScriptPin;
use pins::{
    cig_from_json, cigar_from_toml, header_pin, pin_from_carton_file, pin_from_json, pin_from_toml,
};

/// The tools the shim serves.
pub const TOOLS: [&str; 4] = ["cig", "cigo", "cigcompile", "cigar"];

/// The shim files the installer places in `bin/` and the tool each runs. One tool may have more
/// than one shim: `cigar-lsp` is a shim of `cigar` so that an editor that starts `cigar-lsp` in a
/// project gets the language server of the cigar the project resolves to. A name not in this
/// table is refused (E1844), the same as a name not in [`TOOLS`] before this table existed.
pub const SHIMS: [(&str, &str); 5] = [
    ("cig", "cig"),
    ("cigo", "cigo"),
    ("cigcompile", "cigcompile"),
    ("cigar", "cigar"),
    ("cigar-lsp", "cigar"),
];

/// The tool a shim serves, or `None` when the name is not one of the shims.
pub fn tool_of(shim: &str) -> Option<&'static str> {
    SHIMS.iter().find_map(|(s, t)| (*s == shim).then_some(*t))
}

/// The shims a tool serves (`[("cigar", "cigar-lsp")]` for `cigar`, one shim for every other).
pub fn shims_of(tool: &str) -> Vec<&'static str> {
    SHIMS
        .iter()
        .filter_map(|(s, t)| (*t == tool).then_some(*s))
        .collect()
}
/// The files the Windows self-update and the shim agree on, relative to `CIGSCRIPT_HOME`.
/// `bin\cigi.exe` is the shim there, not a second copy of the installer; when
/// `bin\ciginstaller.exe` cannot start while [`PENDING_CHECK`] is still there, the shim puts
/// [`PREVIOUS_INSTALLER`] back (it is the one thing the shim writes) and leaves [`RESTORED`].
pub const PENDING_CHECK: [&str; 2] = ["installer", "pending-check"];
pub const RESTORED: [&str; 2] = ["installer", "restored"];
pub const PREVIOUS_INSTALLER: [&str; 3] = ["installer", "previous", "ciginstaller.exe"];
/// Where the build that failed to start is kept after a restore, for the report.
pub const FAILED_INSTALLER: [&str; 3] = ["installer", "previous", "ciginstaller.failed.exe"];
/// The most levels one walk climbs.
pub const MAX_DEPTH: usize = 40;
/// The shim's cap on a pin file. A larger one goes to the installer, which takes `MAX_FILE_INSTALLER`.
pub const MAX_FILE: u64 = 64 * 1024;
/// The installer's cap on a pin file.
pub const MAX_FILE_INSTALLER: u64 = 1024 * 1024;

/// Who may own a project pin file: the owner of CIGSCRIPT_HOME, or root. `None` off Unix.
#[derive(Debug, Clone, Copy, Default)]
pub struct Trusted {
    #[cfg_attr(not(unix), allow(dead_code))] // read by the Unix owner check only
    uid: Option<u32>,
    /// The check is on (Unix); a home whose owner cannot be read then trusts no pin file.
    #[cfg_attr(not(unix), allow(dead_code))]
    on: bool,
}

#[cfg(unix)]
pub fn trusted_for(home: &Path) -> Trusted {
    use std::os::unix::fs::MetadataExt;
    Trusted {
        uid: fs::metadata(home).ok().map(|m| m.uid()),
        on: true,
    }
}
#[cfg(not(unix))]
pub fn trusted_for(_home: &Path) -> Trusted {
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

/// Where a shim's home comes from, in the order the shim trusts them (decision 0005).
///
/// A family tool belongs to the home it lives in: a shim at `<home>/bin/<name>`, or a build under
/// `<home>/toolchains/`, works against that home whatever `CIGSCRIPT_HOME` says (the running file's
/// resolved location, so a link to the shim from another folder still finds its own home). A
/// folder counts as a home when it holds `installer/`. `CIGSCRIPT_HOME` (or `~/.cigscript`)
/// chooses the home only for a binary that is not inside one: a development build, the bootstrap
/// installer, a test's binary. `CIGSCRIPT_HOME` stays cig's state root, which cig's box moves to
/// the run's inbox; the installer's settings, toolchains and shims do not move with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomeChoice {
    /// The home the shim works against.
    pub home: PathBuf,
    /// The home its own location implies, when it sits in a `bin` folder.
    pub anchored: Option<PathBuf>,
    /// `CIGSCRIPT_HOME`, when it is set and not empty.
    pub env: Option<PathBuf>,
    /// The home was chosen because the running file lives in it ([`home_containing`]).
    pub owned: bool,
}

impl HomeChoice {
    /// The two homes that disagree (`env`, `anchored`), when they do. The comparison follows
    /// links in both, so a home reached through a symlink agrees with itself.
    pub fn disagreement(&self) -> Option<(&Path, &Path)> {
        let (e, a) = (self.env.as_deref()?, self.anchored.as_deref()?);
        let same = e == a
            || matches!((fs::canonicalize(e), fs::canonicalize(a)), (Ok(x), Ok(y)) if x == y);
        (!same).then_some((e, a))
    }
}

/// `<home>/bin/<name>` gives `<home>`: the parent of a folder called `bin`. A file anywhere else
/// (a test copy, a link target) has no home of its own.
pub fn home_of_exe(exe: &Path) -> Option<PathBuf> {
    let bin = exe.parent()?;
    let named_bin = bin.file_name().is_some_and(|n| {
        if cfg!(windows) {
            n.eq_ignore_ascii_case("bin")
        } else {
            n == "bin"
        }
    });
    let home = bin.parent()?;
    (named_bin && !home.as_os_str().is_empty()).then(|| home.to_path_buf())
}

/// The family home a running file lives in (decision 0005): the nearest folder above `exe` that
/// holds `installer/` and has `exe` under its `bin/` or its `toolchains/`. A shim, the
/// installer's own copy in `bin/` and every installed build answer their home; a development
/// build or a test's binary answers `None`.
pub fn home_containing(exe: &Path) -> Option<PathBuf> {
    let mut dir = exe.parent();
    while let Some(d) = dir {
        let name = d.file_name()?;
        let is = |want: &str| {
            if cfg!(windows) {
                name.eq_ignore_ascii_case(want)
            } else {
                name == want
            }
        };
        if is("bin") || is("toolchains") {
            if let Some(home) = d.parent().filter(|h| !h.as_os_str().is_empty()) {
                if home.join("installer").is_dir() {
                    return Some(home.to_path_buf());
                }
            }
        }
        dir = d.parent();
    }
    None
}

/// The home a shim works against: the home it lives in, else `CIGSCRIPT_HOME`, else the folder
/// above its `bin` when that is not a home yet, else `~/.cigscript`.
pub fn choose_home(
    exe: Option<&Path>,
    env: Option<PathBuf>,
    user_home: Option<PathBuf>,
) -> Option<HomeChoice> {
    let anchored = exe.and_then(home_of_exe);
    let own = exe.and_then(home_containing);
    let owned = own.is_some();
    let home = own
        .or_else(|| env.clone())
        .or_else(|| anchored.clone())
        .or_else(|| user_home.map(|h| h.join(".cigscript")))?;
    Some(HomeChoice {
        home,
        anchored,
        env,
        owned,
    })
}

/// Why the shim will not run what the index names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetError {
    /// The path is not plain: it climbs (`..`), is empty, or names a drive or a root where a
    /// path inside the home belongs.
    NotPlain,
    /// It resolves to somewhere outside the home.
    Outside,
}

/// Turn an index entry into the file to run. A relative entry (the form the installer writes
/// now) is read against `home`; an absolute one (the form older installers wrote) is accepted
/// only when it is already inside `home`. Neither may climb with `..`. Lexical: nothing is read.
pub fn resolve_entry(home: &Path, entry: &str) -> Result<PathBuf, TargetError> {
    use std::path::Component;
    let p = Path::new(entry);
    if entry.is_empty() || p.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(TargetError::NotPlain);
    }
    if p.is_absolute() {
        return if p.starts_with(home) {
            Ok(p.to_path_buf())
        } else {
            Err(TargetError::Outside)
        };
    }
    if p.components()
        .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        // a drive prefix or a rooted path without a drive: not inside the home
        return Err(TargetError::Outside);
    }
    Ok(home.join(p))
}

/// Does `target` (as the file system resolves it, links followed) stay inside `home`? Both are
/// resolved, so a link anywhere along the way cannot lead out of the home unseen.
pub fn stays_inside(home: &Path, target: &Path) -> Option<PathBuf> {
    let h = fs::canonicalize(home).ok()?;
    let t = fs::canonicalize(target).ok()?;
    t.starts_with(&h).then_some(t)
}

/// What the installer records about a build's file and the shim checks before it runs it:
/// `(dev, inode, size, mtime)`. One `stat` per run, no hashing; the receipt's hash is checked by
/// `ciginstaller repair`, never on the hot path. Windows has no inode in std: `dev` and `ino`
/// are 0 there and the size and the write time carry the check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identity {
    pub dev: u64,
    pub ino: u64,
    pub size: u64,
    /// Modification time in nanoseconds since the epoch (100 ns units on Windows).
    pub mtime: i128,
}

impl Identity {
    /// The identity of an already-read `stat`.
    #[cfg(unix)]
    pub fn of(md: &fs::Metadata) -> Identity {
        use std::os::unix::fs::MetadataExt;
        Identity {
            dev: md.dev(),
            ino: md.ino(),
            size: md.size(),
            mtime: i128::from(md.mtime()) * 1_000_000_000 + i128::from(md.mtime_nsec()),
        }
    }
    #[cfg(windows)]
    pub fn of(md: &fs::Metadata) -> Identity {
        use std::os::windows::fs::MetadataExt;
        Identity {
            dev: 0,
            ino: 0,
            size: md.file_size(),
            mtime: i128::from(md.last_write_time()),
        }
    }
    #[cfg(not(any(unix, windows)))]
    pub fn of(md: &fs::Metadata) -> Identity {
        Identity {
            dev: 0,
            ino: 0,
            size: md.len(),
            mtime: 0,
        }
    }

    /// Measure the file at `path` without following a link at its end.
    pub fn measure(path: &Path) -> std::io::Result<Identity> {
        fs::symlink_metadata(path).map(|md| Identity::of(&md))
    }

    /// `dev:ino:size:mtime`, the form the index and the receipt hold.
    pub fn render(&self) -> String {
        format!("{}:{}:{}:{}", self.dev, self.ino, self.size, self.mtime)
    }

    pub fn parse(s: &str) -> Option<Identity> {
        let mut f = s.split(':');
        let id = Identity {
            dev: f.next()?.parse().ok()?,
            ino: f.next()?.parse().ok()?,
            size: f.next()?.parse().ok()?,
            mtime: f.next()?.parse().ok()?,
        };
        f.next().is_none().then_some(id)
    }
}

/// The user's home directory as the shim sees it: `$HOME`, else `$USERPROFILE`.
pub fn user_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// The outcome of reading one pin file.
#[derive(Debug, PartialEq)]
pub enum Reading {
    /// No such file, or not one we may trust (a link, another owner, not a regular file):
    /// the same as no file.
    Absent,
    Text(String),
    /// It exists and is ours to read, but not within the cap, not text, or not readable. The text says why.
    Unusable(String),
}

/// Is `path` something we may trust without opening it: not a link, owned by a trusted user, and a
/// regular file (or a directory when `dir_ok`)? Never opens it.
pub fn trusted_entry(path: &Path, t: Trusted, dir_ok: bool) -> bool {
    fs::symlink_metadata(path).is_ok_and(|md| {
        let ft = md.file_type();
        (ft.is_file() || (dir_ok && ft.is_dir())) && owner_ok(&md, t)
    })
}

/// `O_NONBLOCK` where its value is known: the BSDs and macOS, and Linux on the architectures
/// whose `fcntl.h` is the generic one. Elsewhere `None`, and opening is plain.
#[cfg(unix)]
fn nonblock_flag() -> Option<i32> {
    if cfg!(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly"
    )) {
        Some(0x4)
    } else if cfg!(all(
        any(target_os = "linux", target_os = "android"),
        any(
            target_arch = "x86_64",
            target_arch = "x86",
            target_arch = "aarch64",
            target_arch = "arm",
            target_arch = "riscv64",
            target_arch = "powerpc64",
            target_arch = "s390x",
            target_arch = "loongarch64"
        )
    )) {
        Some(0o4000)
    } else {
        None
    }
}

/// Open a file for reading without blocking. The callers check that the path is a regular file
/// before opening and again on the opened file; this closes the gap between the two, where a
/// fifo swapped in by someone who can write the directory would otherwise hang the open.
/// (`O_NONBLOCK` changes nothing for reading a regular file.)
pub fn open_nonblocking(path: &Path) -> std::io::Result<File> {
    let mut o = fs::OpenOptions::new();
    o.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        if let Some(flag) = nonblock_flag() {
            o.custom_flags(flag);
        }
    }
    o.open(path)
}

/// Read a pin file: only a regular file (never a link, fifo or device, checked before opening and
/// again on the opened file), owned by a trusted user, at most `cap` bytes of UTF-8.
pub fn read_capped(path: &Path, t: Trusted, cap: u64) -> Reading {
    let Ok(before) = fs::symlink_metadata(path) else {
        return Reading::Absent;
    };
    if !before.file_type().is_file() || !owner_ok(&before, t) {
        return Reading::Absent;
    }
    let f = match open_nonblocking(path) {
        Ok(f) => f,
        Err(e) => return Reading::Unusable(format!("it cannot be opened: {e}")),
    };
    let Ok(after) = f.metadata() else {
        return Reading::Unusable("it cannot be inspected".to_string());
    };
    if !after.is_file() || !owner_ok(&after, t) {
        return Reading::Absent;
    }
    // one byte past the cap tells "exactly at the cap" from "over it"
    let mut bytes = Vec::new();
    match f.take(cap + 1).read_to_end(&mut bytes) {
        Err(e) => return Reading::Unusable(format!("it cannot be read: {e}")),
        Ok(_) if bytes.len() as u64 > cap => {
            return Reading::Unusable(format!("it is larger than {cap} bytes"))
        }
        Ok(_) => {}
    }
    String::from_utf8(bytes).map_or_else(
        |_| Reading::Unusable("it is not UTF-8 text".to_string()),
        Reading::Text,
    )
}

/// Which dialect a pin file speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    CartonFile,
    Json,
    Toml,
}

/// What the walk up from a directory found.
#[derive(Debug, PartialEq)]
pub enum Found {
    /// A current pin file decided. `pin` is `None` only for a `.cig-carton` that holds no pin
    /// (a verdict by itself: the resolver says so).
    Pin {
        dir: PathBuf,
        file: PathBuf,
        kind: Kind,
        pin: Option<String>,
    },
    /// No current pin file, but these legacy per-tool files are here and trusted. A `cig-tasks.json`
    /// whose top level has a `"cig"` key (the cig the project is written for) is one of them.
    Legacy {
        dir: PathBuf,
        files: Vec<PathBuf>,
    },
    /// A pin file is there and ours but unusable here (over the cap, not UTF-8, unreadable).
    Unusable {
        dir: PathBuf,
        file: PathBuf,
        why: String,
    },
    Nothing,
}

/// `start` and the home it is searched up to, in one form that can be compared. When `start` is
/// under `stop` as written, nothing is looked up. When it is not, a link in either name can hide
/// that it is (macOS: `/var` is `/private/var`, and a cwd always comes back as the real path), so
/// the home is canonicalized once (a few components) and, only if that is not enough, the start.
/// A start that is still not under the home has no stop: the walk is not bounded by it.
pub fn bound<'a>(
    start: &'a Path,
    stop: Option<&'a Path>,
) -> (
    std::borrow::Cow<'a, Path>,
    Option<std::borrow::Cow<'a, Path>>,
) {
    use std::borrow::Cow::{Borrowed, Owned};
    let Some(h) = stop else {
        return (Borrowed(start), None);
    };
    if start.starts_with(h) {
        return (Borrowed(start), Some(Borrowed(h)));
    }
    match fs::canonicalize(h) {
        Ok(c) if start.starts_with(&c) => (Borrowed(start), Some(Owned(c))),
        Ok(c) => match fs::canonicalize(start) {
            Ok(s) if s.starts_with(&c) => (Owned(s), Some(Owned(c))),
            _ => (Borrowed(start), None),
        },
        Err(_) => (Borrowed(start), None),
    }
}

/// Walk up from `start` (at most 40 levels): the first directory with a verdict decides.
/// In one directory: `.cig-carton`, then `cig-tasks.json`, then `cigar.toml`, then the `legacy` names.
/// A `cig-tasks.json` with a `"cig"` key and a `cigar.toml` with a `cigar` key (and no `carton`)
/// are legacy pins: they come first among the legacy files.
///
/// `stop`: the user's home directory. A project inside it is searched up to and including the home
/// and no further, so `/home/.cig-carton` or `/tmp/.cig-carton` never reaches into it.
pub fn find(start: &Path, legacy: &[&str], t: Trusted, stop: Option<&Path>, cap: u64) -> Found {
    let (start, stop) = bound(start, stop);
    let (start, stop) = (start.as_ref(), stop.as_deref());
    let mut dir = Some(start);
    for _ in 0..MAX_DEPTH {
        let Some(d) = dir else { break };
        let mut manifest_files: Vec<PathBuf> = Vec::new();
        for (name, kind) in [
            (".cig-carton", Kind::CartonFile),
            ("cig-tasks.json", Kind::Json),
            ("cigar.toml", Kind::Toml),
        ] {
            let file = d.join(name);
            match read_capped(&file, t, cap) {
                Reading::Absent => {}
                Reading::Unusable(why) => {
                    return Found::Unusable {
                        dir: d.to_path_buf(),
                        file,
                        why,
                    }
                }
                Reading::Text(text) => {
                    let pin = match kind {
                        Kind::CartonFile => pin_from_carton_file(&text),
                        Kind::Json => pin_from_json(&text),
                        Kind::Toml => pin_from_toml(&text),
                    };
                    // a `.cig-carton` is a verdict by itself; the others only when they pin
                    if kind == Kind::CartonFile || pin.is_some() {
                        return Found::Pin {
                            dir: d.to_path_buf(),
                            file,
                            kind,
                            pin,
                        };
                    }
                    // a manifest's `"cig"` floor and `cigar.toml`'s `cigar` key are pins of the
                    // legacy kind: they send the project to the resolver, never to a carton
                    let names_a_version = match kind {
                        Kind::Json => cig_from_json(&text).is_some(),
                        Kind::Toml => cigar_from_toml(&text).is_some(),
                        Kind::CartonFile => false,
                    };
                    if names_a_version {
                        manifest_files.push(file);
                    }
                }
            }
        }
        let files: Vec<PathBuf> = manifest_files
            .into_iter()
            .chain(
                legacy
                    .iter()
                    .map(|n| d.join(n))
                    .filter(|p| trusted_entry(p, t, false)),
            )
            .collect();
        if !files.is_empty() {
            return Found::Legacy {
                dir: d.to_path_buf(),
                files,
            };
        }
        if stop == Some(d) {
            break;
        }
        dir = d.parent();
    }
    Found::Nothing
}

/// The cap on what the shim reads of a script to find its header: a header sits before the first
/// statement, so a script whose first 16 KiB are still comments has none worth finding.
pub const MAX_SCRIPT_PEEK: u64 = 16 * 1024;

/// The words that put the file to run after them: `cig run s.cig`, `cig check s.cig`.
const SCRIPT_SUBCOMMANDS: [&str; 4] = ["run", "check", "light", "chains"];

/// The argument that names the script a `cig` command is about: the first that is not a flag,
/// after `run`, `check`, `light` or `chains` when one comes first. After a `--` the next argument
/// counts whatever it starts with. `None` when the command has none (`cig --version`).
pub fn script_arg(args: &[std::ffi::OsString]) -> Option<&std::ffi::OsString> {
    let (mut sub, mut rest_are_args) = (false, false);
    for a in args {
        let s = a.to_str();
        if !rest_are_args {
            if s == Some("--") {
                rest_are_args = true;
                continue;
            }
            if s.is_some_and(|s| s.starts_with('-')) {
                continue;
            }
        }
        if !sub && s.is_some_and(|s| SCRIPT_SUBCOMMANDS.contains(&s)) {
            sub = true;
            continue;
        }
        return Some(a);
    }
    None
}

/// The pin in the header of the script at `path`, when it is a readable regular file (a link to
/// one counts: cig reads through links) that carries one. Opens without blocking, reads at most
/// [`MAX_SCRIPT_PEEK`] bytes and never writes. Anything else is `None`: no file, no pin.
pub fn read_script_header(path: &Path) -> Option<ScriptPin> {
    let f = open_nonblocking(path).ok()?;
    if !f.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    f.take(MAX_SCRIPT_PEEK).read_to_end(&mut bytes).ok()?;
    header_pin(&String::from_utf8_lossy(&bytes))
}

#[cfg(test)]
mod lib_tests;
