// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! Compiled programs (#30): finding them by their trailer, saying which have an advisory, and
//! `programs rebuild` through cigcompile.
//!
//! **Where programs are looked for, exactly.** Never a scan of the disk and never recursive:
//!
//! - every absolute entry of `PATH` (a relative or empty entry means "wherever I am" and is skipped);
//! - the nearest project folder from the working directory (the walk-up the shim and `which` use for
//!   pin files), and each `--project <dir>` given;
//! - in a project folder: the files directly in it and directly in its `bin/` (where cigcompile
//!   writes by default).
//!
//! Only regular files are opened (a FIFO or a device never is), read-only, executable on Unix and
//! `.exe` on Windows, and only their last few KiB (and, when a trailer is there, its payload) are
//! read. At most [`MAX_ENTRIES`] entries of one folder are looked at. Nothing is sent anywhere, and
//! the advisories are those of the cached manifest: this never fetches (#19).
//!
//! `rebuild` runs cigcompile and cig the way the installer runs any family tool: only from a build
//! that matches its receipt, with a time limit, from the home and never from the project.

use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use cig_compat::program::{self, Program};
use cig_compat::{Advisory, Manifest};
use cigi_home::state::HistoryEntry;
use cigi_home::{now_rfc3339, verify_toolchain};
use cigi_ui::{Align, Cell, InstallerError, Report, Table, EXIT_FAILED, EXIT_USAGE};
use serde_json::{json, Value};

use super::doctor::{run_args, Ran};
use super::family::installed_version;
use super::{settle, tool_exe};
use crate::cx::Cx;
use crate::errors::ie;

/// The most entries of one folder that are looked at.
pub const MAX_ENTRIES: usize = 20_000;

const BUILD_SECS: u64 = 300;
const VERIFY_SECS: u64 = 120;
const TEST_SECS: u64 = 300;

/// The code of a rebuild that did not happen.
const E_REBUILD: &str = "E1859";

/// The folders looked in.
#[derive(Debug, Clone, Default)]
pub struct Where {
    /// Absolute `PATH` entries, in order, without repeats.
    pub path_dirs: Vec<PathBuf>,
    /// Project folders (each also looked in at `bin/`).
    pub project_dirs: Vec<PathBuf>,
}

/// One program found.
#[derive(Debug, Clone)]
pub struct Found {
    /// Where it was found.
    pub path: PathBuf,
    /// Where it really is (links followed).
    pub real: PathBuf,
    pub program: Program,
}

#[derive(Debug, Default)]
pub struct Scan {
    pub found: Vec<Found>,
    /// Files with a trailer that is damaged, and folders that could not be read.
    pub notes: Vec<String>,
}

/// A program with the advisories of the manifest that cover its cig.
pub struct Hit<'a> {
    pub found: &'a Found,
    pub advisories: Vec<&'a Advisory>,
}

pub fn folders(cx: &Cx, projects: &[PathBuf]) -> Where {
    let mut w = Where::default();
    if let Some(p) = std::env::var_os("PATH") {
        for d in std::env::split_paths(&p) {
            if d.is_absolute() && !w.path_dirs.contains(&d) {
                w.path_dirs.push(d);
            }
        }
    }
    let mut add = |d: PathBuf| {
        if !w.project_dirs.contains(&d) {
            w.project_dirs.push(d);
        }
    };
    if let Ok(Some(p)) = cx.project() {
        add(p.root);
    }
    for d in projects {
        add(if d.is_absolute() {
            d.clone()
        } else {
            cx.cwd.join(d)
        });
    }
    w
}

#[cfg(unix)]
fn runnable(md: &fs::Metadata, _path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    md.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn runnable(_md: &fs::Metadata, path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.to_string_lossy().eq_ignore_ascii_case("exe"))
}

fn look_in(dir: &Path, seen: &mut BTreeSet<PathBuf>, out: &mut Scan) {
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten().take(MAX_ENTRIES) {
        let path = entry.path();
        // follows a link once: what is wanted is the file it names
        let Ok(md) = fs::metadata(&path) else {
            continue;
        };
        if !md.is_file() || md.len() < program::TRAILER_LEN || !runnable(&md, &path) {
            continue;
        }
        let real = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if !seen.insert(real.clone()) {
            continue;
        }
        let Ok(mut f) = fs::File::open(&real) else {
            continue;
        };
        match program::read(&mut f) {
            Ok(Some(program)) => out.found.push(Found {
                path,
                real,
                program,
            }),
            Ok(None) => {}
            Err(why) => out
                .notes
                .push(format!("{} was skipped: {why}", path.display())),
        }
    }
}

/// Look in the folders of `w`.
pub fn scan(w: &Where) -> Scan {
    let mut out = Scan::default();
    let mut seen = BTreeSet::new();
    for d in &w.path_dirs {
        look_in(d, &mut seen, &mut out);
    }
    for d in &w.project_dirs {
        look_in(d, &mut seen, &mut out);
        look_in(&d.join("bin"), &mut seen, &mut out);
    }
    out
}

/// The programs whose cig an advisory of `m` covers.
pub fn hits<'a>(m: &'a Manifest, found: &'a [Found]) -> Vec<Hit<'a>> {
    found
        .iter()
        .filter_map(|f| {
            let v = f.program.cig_version()?;
            let advisories = program::advisories_for_cig(m, &v);
            (!advisories.is_empty()).then_some(Hit {
                found: f,
                advisories,
            })
        })
        .collect()
}

fn shown(cx: &Cx, p: &Path) -> String {
    let home = cx.home.user_home();
    match p.strip_prefix(&home) {
        Ok(rest) if !rest.as_os_str().is_empty() => format!("~/{}", rest.display()),
        _ => p.display().to_string(),
    }
}

fn adv_json(a: &Advisory) -> Value {
    json!({
        "id": a.id,
        "severity": a.severity.as_str(),
        "fixed_in": a.fixed_in.as_ref().map(|v| v.to_string()),
        "summary": a.summary,
    })
}

fn adv_words(advisories: &[&Advisory]) -> String {
    advisories
        .iter()
        .map(|a| {
            let fixed = a
                .fixed_in
                .as_ref()
                .map(|f| format!(", fixed in {f}"))
                .unwrap_or_default();
            format!("{} ({}{fixed})", a.id, a.severity.as_str())
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// `programs [--project DIR]...`
pub fn list(cx: &mut Cx, projects: &[PathBuf]) -> Result<Report, InstallerError> {
    let w = folders(cx, projects);
    for d in projects {
        if !cx.cwd.join(d).is_dir() {
            return Err(InstallerError::new(
                "E1870",
                format!("--project {}: not a folder", d.display()),
            )
            .with_fix("ciginstaller programs --project <folder>")
            .with_exit(EXIT_USAGE));
        }
    }
    let s = scan(&w);
    let manifest = cx.peek_manifest();
    let cig_now = installed_version(&cx.state, "cig");
    let hit = manifest
        .as_ref()
        .map(|m| hits(m, &s.found))
        .unwrap_or_default();
    let looked = json!({
        "path": w.path_dirs.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
        "projects": w.project_dirs.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
    });
    let mut notes: Vec<String> = s.notes.clone();
    if manifest.is_none() && !s.found.is_empty() {
        notes.push(
            "no manifest is cached, so advisories are not known: ciginstaller update --check"
                .to_string(),
        );
    }
    let programs: Vec<Value> = s
        .found
        .iter()
        .map(|f| {
            let advisories: Vec<&Advisory> = hit
                .iter()
                .find(|h| std::ptr::eq(h.found, f))
                .map(|h| h.advisories.clone())
                .unwrap_or_default();
            json!({
                "name": f.program.name,
                "path": f.path.display().to_string(),
                "cig": f.program.cig,
                "compiler": f.program.compiler,
                "script": f.program.file,
                "advisories": advisories.iter().map(|a| adv_json(a)).collect::<Vec<_>>(),
                "needs_rebuild": !advisories.is_empty(),
            })
        })
        .collect();
    let data = json!({
        "programs": programs,
        "cig": cig_now.as_ref().map(|v| v.to_string()),
        "looked": looked,
        "notes": notes,
    });
    if s.found.is_empty() {
        let mut r = Report::done("no compiled programs found")
            .detail(format!(
                "looked on PATH ({} folders) and in {} project folder(s); nothing else is scanned",
                w.path_dirs.len(),
                w.project_dirs.len()
            ))
            .next("ciginstaller programs --project <folder>");
        for n in &notes {
            r = r.detail(n.clone());
        }
        return Ok(r.data(data));
    }
    let mut table = Table::new(&[
        ("program", Align::Left),
        ("built with", Align::Right),
        ("cig now", Align::Right),
        ("advisory", Align::Left),
    ])
    .wide(cx.ui.wide);
    for f in &s.found {
        let advisories = hit
            .iter()
            .find(|h| std::ptr::eq(h.found, f))
            .map(|h| h.advisories.clone())
            .unwrap_or_default();
        let mut label = shown(cx, &f.path);
        if f.path.file_stem().and_then(|n| n.to_str()) != Some(f.program.name.as_str()) {
            label = format!("{label} ({})", f.program.name);
        }
        let built = if advisories.is_empty() {
            Cell::new(f.program.cig.clone())
        } else {
            Cell::changed(f.program.cig.clone())
        };
        table = table.row(vec![
            Cell::new(label),
            built,
            Cell::new(cig_now.as_ref().map_or("—".to_string(), |v| v.to_string())),
            Cell::new(if advisories.is_empty() {
                "—".to_string()
            } else {
                format!("{}: rebuild with cigcompile", adv_words(&advisories))
            }),
        ]);
    }
    let n = s.found.len();
    let need = hit.len();
    let mut r = Report::done(format!(
        "{n} program{}{}",
        if n == 1 { "" } else { "s" },
        if need == 0 {
            String::new()
        } else {
            format!(
                "; {need} {} a rebuild",
                if need == 1 { "needs" } else { "need" }
            )
        }
    ));
    for l in super::super::cmd::tabular::details(&cx.ui, &table) {
        r = r.detail(l);
    }
    if cx.ui.verbose {
        for f in &s.found {
            r = r.detail(format!(
                "{}: {}, script {}",
                f.program.name, f.program.compiler, f.program.file
            ));
        }
    }
    for n in &notes {
        r = r.detail(n.clone());
    }
    if let Some(h) = hit.first() {
        r = r.next(format!(
            "ciginstaller programs rebuild {}",
            h.found.program.name
        ));
    }
    Ok(r.data(data))
}

// ------------------------------------------------------------------ rebuild

#[derive(Debug, Clone, Default)]
pub struct RebuildArgs {
    /// A program's name, or its path.
    pub target: String,
    pub script: Option<PathBuf>,
    pub projects: Vec<PathBuf>,
    pub dry_run: bool,
}

fn refused(why: impl Into<String>) -> InstallerError {
    InstallerError::new(E_REBUILD, why).with_exit(EXIT_FAILED)
}

fn tail(r: &Ran) -> String {
    let t = format!("{}{}", r.stderr, r.stdout);
    let lines: Vec<&str> = t.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let from = lines.len().saturating_sub(3);
    let j = lines[from..].join(" / ");
    if j.is_empty() {
        "it said nothing".to_string()
    } else {
        j.chars().take(400).collect()
    }
}

fn utf8(p: &Path) -> Result<&str, InstallerError> {
    p.to_str().ok_or_else(|| {
        refused(format!(
            "{} is not a path cigcompile can be given as text",
            p.display()
        ))
    })
}

/// The program's script on this machine: `--script`, the record's path as it is, or relative to the
/// program's folder, its parent (a program in `<project>/bin`) and each known project folder.
fn find_script(f: &Found, given: Option<&Path>, w: &Where) -> Option<PathBuf> {
    let good =
        |p: &Path| fs::metadata(p).is_ok_and(|m| m.is_file() && m.len() <= program::MAX_PAYLOAD);
    if let Some(g) = given {
        return good(g).then(|| g.to_path_buf());
    }
    let file = Path::new(&f.program.file);
    if file.is_absolute() {
        return good(file).then(|| file.to_path_buf());
    }
    let dir = f.real.parent()?;
    let mut bases: Vec<PathBuf> = vec![dir.to_path_buf()];
    if let Some(up) = dir.parent() {
        bases.push(up.to_path_buf());
    }
    bases.extend(w.project_dirs.iter().cloned());
    bases
        .into_iter()
        .map(|b| b.join(file))
        // a record's path never climbs out of the folder it is read against
        .filter(|p| {
            !p.components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        })
        .find(|p| good(p))
}

fn sha256_file(p: &Path) -> Option<String> {
    use sha2::{Digest, Sha256};
    let mut f = fs::File::open(p).ok()?;
    let mut h = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Some(hex::encode(h.finalize()))
}

pub fn rebuild(cx: &mut Cx, a: &RebuildArgs) -> Result<Report, InstallerError> {
    for d in &a.projects {
        if !cx.cwd.join(d).is_dir() {
            return Err(InstallerError::new(
                "E1870",
                format!("--project {}: not a folder", d.display()),
            )
            .with_fix("ciginstaller programs --project <folder>")
            .with_exit(EXIT_USAGE));
        }
    }
    let w = folders(cx, &a.projects);
    let s = scan(&w);
    let by_path = a.target.contains('/') || a.target.contains('\\');
    let want = if by_path {
        fs::canonicalize(cx.cwd.join(&a.target)).ok()
    } else {
        None
    };
    let matches: Vec<&Found> = s
        .found
        .iter()
        .filter(|f| {
            if by_path {
                want.as_ref().is_some_and(|p| *p == f.real)
            } else {
                f.program.name == a.target
            }
        })
        .collect();
    let found = match matches.as_slice() {
        [one] => *one,
        [] => {
            return Err(InstallerError::new(
                "E1870",
                format!(
                    "no compiled program `{}` on PATH or in this project's folders",
                    a.target
                ),
            )
            .with_hint("nothing else is scanned; `--project <folder>` adds a folder")
            .with_fix("ciginstaller programs")
            .with_exit(EXIT_USAGE))
        }
        many => {
            let paths: Vec<String> = many.iter().map(|f| f.path.display().to_string()).collect();
            return Err(InstallerError::new(
                "E1870",
                format!(
                    "`{}` names {} programs: {}",
                    a.target,
                    many.len(),
                    paths.join(", ")
                ),
            )
            .with_fix(format!("ciginstaller programs rebuild {}", paths[0]))
            .with_exit(EXIT_USAGE));
        }
    };
    let name = found.program.name.clone();
    let manifest = cx.peek_manifest();
    let mine: Vec<&Advisory> = match (&manifest, found.program.cig_version()) {
        (Some(m), Some(v)) => program::advisories_for_cig(m, &v),
        _ => Vec::new(),
    };
    // the script, or what its author needs to be told
    let Some(script) = find_script(found, a.script.as_deref(), &w) else {
        let mut e = refused(format!(
            "the script of {name} is not on this machine (it was compiled from {}), so it cannot be rebuilt here",
            found.program.file
        ))
        .with_fix(format!(
            "ciginstaller programs rebuild {name} --script <the script's path>"
        ));
        let told = if mine.is_empty() {
            format!(
                "{name} was built on cig {} with {}",
                found.program.cig, found.program.compiler
            )
        } else {
            format!(
                "{name} was built on cig {}: {}. Its author needs to rebuild it with a fixed cig",
                found.program.cig,
                adv_words(&mine)
            )
        };
        e = e.with_hint(told);
        return Err(e);
    };
    let cig_ver = installed_version(&cx.state, "cig").ok_or_else(|| {
        refused("cig is not installed, so there is no runtime to build on")
            .with_fix("ciginstaller install carton")
    })?;
    let cc_ver = installed_version(&cx.state, "cigcompile").ok_or_else(|| {
        refused("cigcompile is not installed, so nothing can rebuild a program")
            .with_fix("ciginstaller install carton")
    })?;
    for (tool, v) in [("cig", &cig_ver), ("cigcompile", &cc_ver)] {
        if !verify_toolchain(&cx.home, tool, v).is_empty() {
            return Err(refused(format!(
                "{tool} {v} differs from its receipt, so it is not run"
            ))
            .with_fix("ciginstaller repair"));
        }
    }
    let (Some(cig_exe), Some(cc_exe)) = (
        tool_exe(&cx.home, "cig", &cig_ver),
        tool_exe(&cx.home, "cigcompile", &cc_ver),
    ) else {
        return Err(
            refused("cig or cigcompile has no readable receipt, so it is not run")
                .with_fix("ciginstaller repair"),
        );
    };
    // the runtime must be the fixed one: not covered by an advisory that covers the program
    if let Some(m) = &manifest {
        let still = program::advisories_for_cig(m, &cig_ver);
        let same: Vec<&Advisory> = still
            .iter()
            .copied()
            .filter(|x| mine.iter().any(|y| y.id == x.id))
            .collect();
        if !same.is_empty() {
            return Err(refused(format!(
                "the installed cig {cig_ver} is itself covered by {}, so a rebuild would not fix {name}",
                adv_words(&same)
            ))
            .with_fix("ciginstaller update"));
        }
    }
    if found.program.cig == cig_ver.to_string() && mine.is_empty() {
        return Ok(Report::done(format!(
            "{name} already runs on the installed cig {cig_ver} and nothing applies to it"
        ))
        .data(json!({ "name": name, "rebuilt": false, "reason": "up to date", "cig": cig_ver.to_string() })));
    }
    let real = found.real.clone();
    let script_sha = sha256_file(&script).unwrap_or_default();
    if a.dry_run {
        return Ok(Report::done(format!(
            "would rebuild {name} from {} with cigcompile {cc_ver} on cig {cig_ver}",
            script.display()
        ))
        .detail(format!(
            "then check it with cigcompile verify and cigcompile test, and only then swap {} for it",
            found.path.display()
        ))
        .data(json!({
            "name": name, "dry_run": true, "rebuilt": false,
            "script": script.display().to_string(), "path": found.path.display().to_string(),
            "cig_from": found.program.cig, "cig_to": cig_ver.to_string(),
        })));
    }
    // a program never runs with raised rights: one that has them is not swapped
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(md) = fs::metadata(&real) {
            if md.permissions().mode() & 0o6000 != 0 {
                return Err(refused(format!(
                    "{} is setuid or setgid, and a program never runs with raised rights, so it is not replaced",
                    real.display()
                )));
            }
        }
    }
    cx.lock_and_recover()?;
    let mut txn = cx.begin(&format!("programs rebuild {name}"))?;
    let work = txn.dir().join("rebuild");
    let outcome = (|| -> Result<(String, Vec<String>), InstallerError> {
        fs::create_dir_all(&work)
            .map_err(|e| refused(format!("cannot make {}: {e}", work.display())))?;
        let staged = work.join(&name);
        let tests = work.join("test");
        let root = cx.home.root();
        let (script_s, cig_s) = (utf8(&script)?, utf8(&cig_exe)?);
        let (staged_s, tests_s) = (utf8(&staged)?, utf8(&tests)?);
        let step = |what: &str, args: &[&str], secs: u64| -> Result<(), InstallerError> {
            match run_args(&cc_exe, args, &root, secs, None) {
                Ok(r) if r.code == Some(0) => Ok(()),
                Ok(r) => Err(refused(format!(
                    "`cigcompile {what}` failed ({}): {}; {name} is as it was",
                    r.code
                        .map_or("ended by a signal".to_string(), |c| format!("exit {c}")),
                    tail(&r)
                ))),
                Err(why) => Err(refused(format!(
                    "`cigcompile {what}` could not finish: {why}; {name} is as it was"
                ))),
            }
        };
        step(
            "build",
            &["build", script_s, "--out", staged_s, "--runtime", cig_s],
            BUILD_SECS,
        )?;
        step(
            "verify",
            &["verify", staged_s, "--script", script_s, "--runtime", cig_s],
            VERIFY_SECS,
        )?;
        step(
            "test",
            &["test", script_s, "--out", tests_s, "--runtime", cig_s],
            TEST_SECS,
        )?;
        // what was built is what was asked for
        let new = fs::read(&staged)
            .map_err(|e| refused(format!("the rebuilt program cannot be read: {e}")))?;
        let rec = program::read(&mut std::io::Cursor::new(&new))
            .map_err(|why| refused(format!("the rebuilt program is damaged: {why}")))?
            .ok_or_else(|| refused("cigcompile wrote a file that is not a program"))?;
        if rec.name != name {
            return Err(refused(format!(
                "the rebuilt program is named {}, not {name}; the script's name decides it, so give --script the original script",
                rec.name
            )));
        }
        if rec.cig != cig_ver.to_string() {
            return Err(refused(format!(
                "the rebuilt program is for cig {}, not the installed {cig_ver}",
                rec.cig
            )));
        }
        if !script_sha.is_empty() && rec.sha256 != script_sha {
            return Err(refused(
                "the rebuilt program does not carry the script it was built from",
            ));
        }
        // only now is the old one touched, and the journal puts it back on any failure
        let mode = old_mode(&real);
        txn.write(&real, &new, mode).map_err(ie)?;
        let back = fs::File::open(&real)
            .map_err(|e| refused(format!("{} cannot be read back: {e}", real.display())))
            .and_then(|mut f| {
                program::read(&mut f)
                    .map_err(|why| refused(format!("{} reads back damaged: {why}", real.display())))
            })?;
        if back.as_ref() != Some(&rec) {
            return Err(refused(format!(
                "{} does not read back as the rebuilt program",
                real.display()
            )));
        }
        let mut notes = Vec::new();
        if found.path != real {
            notes.push(format!(
                "{} is a link; the program it points at, {}, was replaced",
                found.path.display(),
                real.display()
            ));
        }
        Ok((rec.compiler, notes))
    })();
    let _ = fs::remove_dir_all(&work);
    let outcome = outcome.and_then(|(compiler, notes)| {
        let mut st = cx.state.clone();
        let id = txn.id().to_string();
        st.push_history(HistoryEntry {
            id: id.clone(),
            at: now_rfc3339(),
            what: "programs".into(),
            from: Some(format!("{name} on cig {}", found.program.cig)),
            to: Some(format!("{name} on cig {cig_ver}")),
            txn: id,
            ok: true,
        });
        st.save_in(&mut txn).map_err(ie)?;
        Ok((compiler, notes, st))
    });
    let (compiler, notes, st) = settle(txn, outcome)?;
    cx.state = st;
    let mut r = Report::done(format!(
        "{name} is rebuilt on cig {cig_ver} ({compiler}), checked with cigcompile verify and cigcompile test"
    ));
    r = r.detail(format!("replaced {}", real.display()));
    if !mine.is_empty() {
        r = r.detail(format!("no longer covered by {}", adv_words(&mine)));
    }
    for n in &notes {
        r = r.detail(n.clone());
    }
    Ok(r.data(json!({
        "name": name,
        "rebuilt": true,
        "path": real.display().to_string(),
        "cig_from": found.program.cig,
        "cig_to": cig_ver.to_string(),
        "compiler": compiler,
        "script": script.display().to_string(),
        "advisories_cleared": mine.iter().map(|a| a.id.clone()).collect::<Vec<_>>(),
    })))
}

#[cfg(unix)]
fn old_mode(p: &Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(p).ok().map(|m| m.permissions().mode() & 0o777)
}

#[cfg(not(unix))]
fn old_mode(_p: &Path) -> Option<u32> {
    None
}

/// For `status` and `doctor`: the programs on PATH and in this project that an advisory of the
/// cached manifest covers. Reads the last few KiB of each executable, and only when the manifest
/// has an advisory about a cig; never fetches.
pub fn advisory_hits(cx: &Cx) -> Vec<(Found, Vec<Advisory>)> {
    let Some(m) = cx.peek_manifest() else {
        return Vec::new();
    };
    // no advisory about a cig, nothing to look for: the folders are not read at all
    if !m
        .advisories
        .iter()
        .any(|a| a.affects.iter().any(|x| x.subject == "cig"))
    {
        return Vec::new();
    }
    let s = scan(&folders(cx, &[]));
    hits(&m, &s.found)
        .into_iter()
        .map(|h| {
            (
                h.found.clone(),
                h.advisories.into_iter().cloned().collect::<Vec<_>>(),
            )
        })
        .collect()
}

/// One line for a program with advisories, as `status` and `doctor` say it: the program, where
/// it is, the cig it runs on and the advisories with their severity and fix.
pub fn hit_line(cx: &Cx, f: &Found, advisories: &[Advisory]) -> String {
    let refs: Vec<&Advisory> = advisories.iter().collect();
    format!(
        "program {} ({}) is built on cig {}, which {} {}",
        f.program.name,
        shown(cx, &f.path),
        f.program.cig,
        if advisories.len() == 1 { "has" } else { "have" },
        adv_words(&refs)
    )
}
