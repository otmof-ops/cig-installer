// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! The trial (SPEC 12.9): before an update switches, run the changed tools' dry runs on the
//! project in scope, old build and new build side by side, and say what plans differently.
//!
//! Nothing here touches the real home: each run gets a scratch `CIGSCRIPT_HOME` and
//! `XDG_CACHE_HOME`, so a dry run cannot write a tin entry or a run record.

use std::collections::VecDeque;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

use cig_compat::Version;
use cigi_shim::{trusted_entry, Trusted};

use crate::project::ProjectCtx;

/// Time one dry run may take.
pub const RUN_LIMIT: Duration = Duration::from_secs(20);
/// Time the whole trial may take; scripts left over are reported as not run.
pub const TOTAL_BUDGET: Duration = Duration::from_secs(120);
/// Scripts one trial runs, newest first.
pub const MAX_SCRIPTS: usize = 50;
/// A script larger than this is not run.
pub const MAX_SCRIPT_BYTES: u64 = 1024 * 1024;
/// Directories below the project root that the search enters.
const MAX_DEPTH: usize = 8;
/// Directory entries looked at before the search stops.
const MAX_VISITED: usize = 20_000;
/// Bytes kept of one stream of one run.
const MAX_CAPTURE: usize = 1024 * 1024;

/// One side of the comparison: a tool build that is installed and can be run.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub tool: String,
    pub carton: String,
    pub version: Version,
    pub path: PathBuf,
    /// The cig of the same carton, when the tool needs one beside it: cigo finds it first on
    /// `PATH`, cigcompile is handed it as `--runtime`.
    pub cig: Option<PathBuf>,
}

/// One script that plans differently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Difference {
    /// Relative to the project root, forward slashes.
    pub script: String,
    /// One line: what differs (`exit 0 -> 1`, `output`).
    pub what: String,
    /// The unified diff of the two normalized outputs.
    pub diff: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrialOutcome {
    pub compared: usize,
    pub same: usize,
    pub different: Vec<Difference>,
    pub skipped: Vec<String>,
}

impl TrialOutcome {
    pub fn merge(&mut self, o: TrialOutcome) {
        self.compared += o.compared;
        self.same += o.same;
        self.different.extend(o.different);
        self.skipped.extend(o.skipped);
    }
}

/// A tool's trial. `run` never fails: what cannot be compared is `skipped`, with the reason.
pub trait Trial {
    fn tool(&self) -> &str;
    fn run(
        &self,
        old: &Resolved,
        new: &Resolved,
        project: &ProjectCtx,
        budget: Duration,
    ) -> TrialOutcome;
}

/// The trials of this build: cig's, cigo's and CigCompile's.
pub fn registered(trust: Trusted) -> Vec<Box<dyn Trial>> {
    vec![
        Box::new(CigTrial { trust }),
        Box::new(CigoTrial { trust }),
        Box::new(CigcompileTrial { trust }),
    ]
}

/// Run the registered trial of each changed tool (`pairs` is old and new per tool).
pub fn run_all(
    trials: &[Box<dyn Trial>],
    pairs: &[(Resolved, Resolved)],
    project: &ProjectCtx,
    budget: Duration,
) -> TrialOutcome {
    let start = Instant::now();
    let mut total = TrialOutcome::default();
    for t in trials {
        if let Some((old, new)) = pairs.iter().find(|(o, _)| o.tool == t.tool()) {
            let left = budget.saturating_sub(start.elapsed());
            total.merge(t.run(old, new, project, left));
        }
    }
    total
}

/// One thing a trial runs on both builds: the same arguments, in `dir`.
struct Probe {
    label: String,
    args: Vec<String>,
    dir: PathBuf,
}

/// Run each probe with the old and then the new build and record what differs. Stops, and says
/// so, when the budget runs out. A probe whose run cannot start or finishes late is skipped with
/// the reason; it never fails the trial.
fn run_probes(
    tool: &str,
    probes: &[Probe],
    old: &Resolved,
    new: &Resolved,
    scratch: &Scratch,
    budget: Duration,
    out: &mut TrialOutcome,
) {
    let start = Instant::now();
    for (i, p) in probes.iter().enumerate() {
        let left = budget.saturating_sub(start.elapsed());
        if left.is_zero() {
            out.skipped.push(format!(
                "{tool}: {} run(s) not made: the trial budget of {}s ran out",
                probes.len() - i,
                budget.as_secs()
            ));
            break;
        }
        let limit = RUN_LIMIT.min(left);
        let a = exec(old, &p.args, &p.dir, &scratch.0, limit);
        let b = exec(new, &p.args, &p.dir, &scratch.0, limit);
        let (a, b) = match (a, b) {
            (Ok(a), Ok(b)) => (a, b),
            (Err(e), _) => {
                out.skipped.push(format!(
                    "{tool}: {}: the old {tool} {}: {e}",
                    p.label, old.version
                ));
                continue;
            }
            (_, Err(e)) => {
                out.skipped.push(format!(
                    "{tool}: {}: the new {tool} {}: {e}",
                    p.label, new.version
                ));
                continue;
            }
        };
        out.compared += 1;
        match difference(&p.label, tool, old, new, &a, &b, &scratch.0) {
            None => out.same += 1,
            Some(d) => out.different.push(d),
        }
    }
}

/// Task names a project's `cig-tasks.json` declares at the top level, for the chains the cigo
/// trial plans. Only plain names are used (`[A-Za-z0-9_.:-]`, not starting with `-`).
fn declared_tasks(manifest: &Path) -> Vec<String> {
    let Ok(md) = std::fs::metadata(manifest) else {
        return Vec::new();
    };
    if md.len() > MAX_SCRIPT_BYTES {
        return Vec::new();
    }
    let Ok(text) = std::fs::read_to_string(manifest) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    v.get("tasks")
        .and_then(|t| t.as_object())
        .map(|o| {
            o.keys()
                .filter(|k| {
                    !k.is_empty()
                        && !k.starts_with('-')
                        && k.chars()
                            .all(|c| c.is_ascii_alphanumeric() || "_.:-".contains(c))
                })
                .take(MAX_CHAINS)
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// Chains the cigo trial plans besides `ci`, the whole pipeline.
pub const MAX_CHAINS: usize = 6;

/// cigo's trial: `cigo check` on the project's `cig-tasks.json`, then a dry run of the `ci` chain
/// and of each task the manifest declares (`cigo --dry-run run <chain>`: every task planned, none
/// executed), old build and new build, compared as cig's are. A project without a
/// `cig-tasks.json` has nothing for cigo to read and is skipped.
pub struct CigoTrial {
    pub trust: Trusted,
}

impl Trial for CigoTrial {
    fn tool(&self) -> &str {
        "cigo"
    }
    fn run(
        &self,
        old: &Resolved,
        new: &Resolved,
        project: &ProjectCtx,
        budget: Duration,
    ) -> TrialOutcome {
        let mut out = TrialOutcome::default();
        let manifest = project.root.join("cig-tasks.json");
        if !trusted_entry(&manifest, self.trust, false) {
            out.skipped
                .push("cigo: no cig-tasks.json in the project".to_string());
            return out;
        }
        let mut probes = vec![Probe {
            label: "cigo check".to_string(),
            args: vec!["check".to_string()],
            dir: project.root.clone(),
        }];
        let mut chains = vec!["ci".to_string()];
        chains.extend(declared_tasks(&manifest).into_iter().filter(|t| t != "ci"));
        for c in chains {
            probes.push(Probe {
                label: format!("cigo --dry-run run {c}"),
                args: ["--dry-run", "--plain", "--no-color", "--no-doctor", "run"]
                    .iter()
                    .map(|s| s.to_string())
                    .chain([c])
                    .collect(),
                dir: project.root.clone(),
            });
        }
        let scratch = match Scratch::new() {
            Ok(s) => s,
            Err(e) => {
                out.skipped
                    .push(format!("cigo: cannot make a scratch directory: {e}"));
                return out;
            }
        };
        run_probes("cigo", &probes, old, new, &scratch, budget, &mut out);
        out
    }
}

/// Scripts the CigCompile trial builds and tests; building is heavier than a dry run.
pub const MAX_COMPILED: usize = 10;

/// CigCompile's trial: `cigcompile test` on the person's own `.cig` programs with each build
/// (it compiles the script, then compares the script's dry run with the program's: same plan,
/// output and exit). The programs go to the trial's scratch folder, never into the project.
/// The two builds' answers are compared, and a script that tests differently on the new one is
/// shown first.
pub struct CigcompileTrial {
    pub trust: Trusted,
}

impl Trial for CigcompileTrial {
    fn tool(&self) -> &str {
        "cigcompile"
    }
    fn run(
        &self,
        old: &Resolved,
        new: &Resolved,
        project: &ProjectCtx,
        budget: Duration,
    ) -> TrialOutcome {
        let mut out = TrialOutcome::default();
        let (mut scripts, truncated) = find_scripts(&project.root, self.trust);
        if truncated {
            out.skipped.push(format!(
                "cigcompile: the project has more than {MAX_VISITED} entries; only what the search reached was sampled"
            ));
        }
        if scripts.is_empty() {
            out.skipped
                .push("cigcompile: no .cig programs found in the project".to_string());
            return out;
        }
        if scripts.len() > MAX_COMPILED {
            out.skipped.push(format!(
                "cigcompile: {} program(s) not tested: the trial tests the newest {MAX_COMPILED}",
                scripts.len() - MAX_COMPILED
            ));
            scripts.truncate(MAX_COMPILED);
        }
        let scratch = match Scratch::new() {
            Ok(s) => s,
            Err(e) => {
                out.skipped
                    .push(format!("cigcompile: cannot make a scratch directory: {e}"));
                return out;
            }
        };
        // the same output path for both builds, so a line that names it cannot differ; the old
        // build runs first and the new one overwrites
        let probes: Vec<Probe> = scripts
            .iter()
            .enumerate()
            .map(|(i, script)| {
                let name = script
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                let target = scratch.0.join("tmp").join(format!("program-{i}"));
                let mut args = vec![
                    "test".to_string(),
                    "--out".to_string(),
                    target.to_string_lossy().to_string(),
                ];
                args.push(format!("./{name}"));
                Probe {
                    label: format!("{} (cigcompile test)", rel_label(&project.root, script)),
                    args,
                    dir: script.parent().unwrap_or(Path::new(".")).to_path_buf(),
                }
            })
            .collect();
        run_probes("cigcompile", &probes, old, new, &scratch, budget, &mut out);
        out
    }
}

// ------------------------------------------------------------------------------------ cig

/// `trust` is who may own the scripts it runs: anchored on the installer home's owner, as pin files
/// are (`cigi_shim::trusted_for(home root)`), whatever directory the project is in.
pub struct CigTrial {
    pub trust: Trusted,
}

impl Trial for CigTrial {
    fn tool(&self) -> &str {
        "cig"
    }

    fn run(
        &self,
        old: &Resolved,
        new: &Resolved,
        project: &ProjectCtx,
        budget: Duration,
    ) -> TrialOutcome {
        let mut out = TrialOutcome::default();
        let (scripts, truncated) = find_scripts(&project.root, self.trust);
        if truncated {
            out.skipped.push(format!(
                "cig: the project has more than {MAX_VISITED} entries; only what the search reached was sampled, so the newest scripts may be missing"
            ));
        }
        if scripts.is_empty() {
            out.skipped
                .push("cig: no .cig scripts found in the project".to_string());
            return out;
        }
        let scratch = match Scratch::new() {
            Ok(s) => s,
            Err(e) => {
                out.skipped
                    .push(format!("cig: cannot make a scratch directory: {e}"));
                return out;
            }
        };
        let start = Instant::now();
        for (i, script) in scripts.iter().enumerate() {
            let left = budget.saturating_sub(start.elapsed());
            if left.is_zero() {
                out.skipped.push(format!(
                    "cig: {} script(s) not run: the trial budget of {}s ran out",
                    scripts.len() - i,
                    budget.as_secs()
                ));
                break;
            }
            let rel = rel_label(&project.root, script);
            let limit = RUN_LIMIT.min(left);
            let a = run_one(old, script, &scratch.0, limit);
            let b = run_one(new, script, &scratch.0, limit);
            let (a, b) = match (a, b) {
                (Ok(a), Ok(b)) => (a, b),
                (Err(e), _) => {
                    out.skipped
                        .push(format!("cig: {rel}: the old cig {}: {e}", old.version));
                    continue;
                }
                (_, Err(e)) => {
                    out.skipped
                        .push(format!("cig: {rel}: the new cig {}: {e}", new.version));
                    continue;
                }
            };
            out.compared += 1;
            match difference(&rel, "cig", old, new, &a, &b, &scratch.0) {
                None => out.same += 1,
                Some(d) => out.different.push(d),
            }
        }
        out
    }
}

/// Compare two runs of one probe. `None` when exit code and both normalized streams are equal;
/// else the difference, with what differs in one line and the unified diffs.
fn difference(
    label: &str,
    tool: &str,
    old: &Resolved,
    new: &Resolved,
    a: &RunOutput,
    b: &RunOutput,
    scratch: &Path,
) -> Option<Difference> {
    let strip = [old.version.to_string(), new.version.to_string()];
    let scratch_s = scratch.to_string_lossy().to_string();
    let (ao, bo) = (
        normalize(&a.stdout, &strip, &scratch_s),
        normalize(&b.stdout, &strip, &scratch_s),
    );
    let (ae, be) = (
        normalize(&a.stderr, &strip, &scratch_s),
        normalize(&b.stderr, &strip, &scratch_s),
    );
    if a.exit == b.exit && ao == bo && ae == be {
        return None;
    }
    let mut what = Vec::new();
    if a.exit != b.exit {
        what.push(format!("exit {} -> {}", code(a.exit), code(b.exit)));
    }
    if ao != bo {
        what.push("stdout".to_string());
    }
    if ae != be {
        what.push(if tool == "cig" {
            "plan and diagnostics (stderr)".to_string()
        } else {
            "stderr".to_string()
        });
    }
    let ol = format!("{tool} {}", old.version);
    let nl = format!("{tool} {}", new.version);
    let mut diff = String::new();
    if ao != bo {
        diff.push_str(&unified_diff(
            &format!("{ol} stdout"),
            &format!("{nl} stdout"),
            &ao,
            &bo,
        ));
    }
    if ae != be {
        diff.push_str(&unified_diff(
            &format!("{ol} stderr"),
            &format!("{nl} stderr"),
            &ae,
            &be,
        ));
    }
    if a.exit != b.exit {
        diff.push_str(&format!(
            "exit code: {} -> {}\n",
            code(a.exit),
            code(b.exit)
        ));
    }
    Some(Difference {
        script: label.to_string(),
        what: what.join(", "),
        diff,
    })
}

fn code(c: Option<i32>) -> String {
    c.map_or("signal".to_string(), |c| c.to_string())
}

fn rel_label(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// A scratch directory, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    /// A new directory with a random name in the temp dir, made non-recursively with mode 0700
    /// (a name that already exists is refused and another is drawn), and the directories a child
    /// needs inside it.
    fn new() -> std::io::Result<Scratch> {
        let mut last = None;
        for _ in 0..8 {
            let p = std::env::temp_dir().join(format!("cigi-trial-{}", random_name()));
            match make_private_dir(&p) {
                Ok(()) => {
                    let s = Scratch(p);
                    for d in ["home", "user", "cache", "data", "state", "config", "tmp"] {
                        make_private_dir(&s.0.join(d))?;
                    }
                    return Ok(s);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => last = Some(e),
                Err(e) => return Err(e),
            }
        }
        Err(last.unwrap_or_else(|| std::io::Error::other("no free scratch name")))
    }
}

#[cfg(unix)]
fn make_private_dir(p: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().mode(0o700).create(p)
}
#[cfg(not(unix))]
fn make_private_dir(p: &Path) -> std::io::Result<()> {
    std::fs::create_dir(p)
}

/// 16 hex digits from the system's random source (the clock and pid only where there is none).
fn random_name() -> String {
    let mut b = [0u8; 8];
    let got = std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut b))
        .is_ok();
    if !got {
        let n = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
            ^ (u64::from(std::process::id()) << 32);
        b = n.to_le_bytes();
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The `*.cig` files of a project the trial may run: regular files we may trust, under 1 MB,
/// not in `.git`, `node_modules`, `target` or a hidden directory; newest first, at most 50.
pub fn find_scripts(root: &Path, trust: Trusted) -> (Vec<PathBuf>, bool) {
    let mut truncated = false;
    let mut found: Vec<(SystemTime, PathBuf)> = Vec::new();
    let mut queue: VecDeque<(PathBuf, usize)> = VecDeque::new();
    queue.push_back((root.to_path_buf(), 0));
    let mut visited = 0usize;
    while let Some((dir, depth)) = queue.pop_front() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            visited += 1;
            if visited > MAX_VISITED {
                truncated = true;
                queue.clear();
                break;
            }
            let path = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            let Ok(md) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            let ft = md.file_type();
            if ft.is_dir() {
                let skip = name.starts_with('.') || name == "node_modules" || name == "target";
                if !skip && depth < MAX_DEPTH {
                    queue.push_back((path, depth + 1));
                }
            } else if ft.is_file()
                && name.ends_with(".cig")
                && md.len() <= MAX_SCRIPT_BYTES
                && trusted_entry(&path, trust, false)
            {
                found.push((md.modified().unwrap_or(SystemTime::UNIX_EPOCH), path));
            }
        }
    }
    found.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    found.truncate(MAX_SCRIPTS);
    (found.into_iter().map(|(_, p)| p).collect(), truncated)
}

struct RunOutput {
    exit: Option<i32>,
    stdout: String,
    stderr: String,
}

fn spawn_retrying(cmd: &mut Command) -> std::io::Result<Child> {
    // a binary written a moment ago can still be open for writing in a child another thread
    // forked, until that child execs: "text file busy"
    let mut tries = 0;
    loop {
        match cmd.spawn() {
            Err(e) if e.raw_os_error() == Some(26) && tries < 100 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(5));
            }
            r => return r,
        }
    }
}

fn drain<R: Read + Send + 'static>(r: Option<R>) -> std::sync::mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut kept = Vec::new();
        if let Some(mut r) = r {
            let mut buf = [0u8; 8192];
            while let Ok(n) = r.read(&mut buf) {
                if n == 0 {
                    break;
                }
                // keep reading past the cap so the child never blocks on a full pipe
                if kept.len() < MAX_CAPTURE {
                    let room = MAX_CAPTURE - kept.len();
                    kept.extend_from_slice(&buf[..n.min(room)]);
                }
            }
        }
        let _ = tx.send(kept);
    });
    rx
}

/// `<exe> run --dry-run --plain --no-color --no-doctor <script>` in the script's directory, with
/// a scratch home, stdin closed and a time limit.
/// The command that starts a cig for the trial. On Unix it runs under `sh`'s limits (CPU seconds
/// and address space, so a hostile project cannot make a dry run eat the machine) and `exec`s
/// itself, so the process the deadline kills is cig itself, not a shell above it. Without
/// `/bin/sh`, cig is started directly.
fn limited(cig: &Path) -> Command {
    #[cfg(unix)]
    {
        if Path::new("/bin/sh").is_file() {
            let mut c = Command::new("/bin/sh");
            c.arg("-c")
                .arg(r#"ulimit -t 60 2>/dev/null; ulimit -v 4194304 2>/dev/null; exec "$0" "$@""#)
                .arg(cig);
            return c;
        }
    }
    Command::new(cig)
}

fn run_one(
    who: &Resolved,
    script: &Path,
    scratch: &Path,
    limit: Duration,
) -> Result<RunOutput, String> {
    let dir = script.parent().unwrap_or(Path::new("."));
    let name = script
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    // "./name": a file called `-x.cig` is a path, not a flag
    let args: Vec<String> = ["run", "--dry-run", "--plain", "--no-color", "--no-doctor"]
        .iter()
        .map(|s| s.to_string())
        .chain([format!("./{name}")])
        .collect();
    exec(who, &args, dir, scratch, limit)
}

/// One run of a tool build: `args` in `dir`, with a scratch home, a short allow-list of the
/// environment (the cig of the same carton first on `PATH` when `who.cig` names one), stdin closed
/// and a time limit.
fn exec(
    who: &Resolved,
    args: &[String],
    dir: &Path,
    scratch: &Path,
    limit: Duration,
) -> Result<RunOutput, String> {
    let mut cmd = limited(&who.path);
    cmd.env_clear();
    // a short allow-list from the parent; everything else of the user's environment stays out
    for k in ["PATH", "LANG", "LC_ALL", "TZ"] {
        if let Some(v) = std::env::var_os(k) {
            cmd.env(k, v);
        }
    }
    if let Some(cig_dir) = who.cig.as_deref().and_then(Path::parent) {
        let rest = std::env::var_os("PATH").unwrap_or_default();
        let joined = std::env::join_paths(
            std::iter::once(cig_dir.to_path_buf()).chain(std::env::split_paths(&rest)),
        );
        if let Ok(j) = joined {
            cmd.env("PATH", j);
        }
    }
    cmd.env(
        "TERM",
        std::env::var_os("TERM").unwrap_or_else(|| "dumb".into()),
    );
    cmd.args(args)
        .current_dir(dir)
        .env("HOME", scratch.join("user"))
        .env("CIGSCRIPT_HOME", scratch.join("home"))
        .env("XDG_CACHE_HOME", scratch.join("cache"))
        .env("XDG_DATA_HOME", scratch.join("data"))
        .env("XDG_STATE_HOME", scratch.join("state"))
        .env("XDG_CONFIG_HOME", scratch.join("config"))
        .env("TMPDIR", scratch.join("tmp"))
        .env("CIGI_CARTON", &who.carton)
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = spawn_retrying(&mut cmd).map_err(|e| format!("did not start: {e}"))?;
    let out = drain(child.stdout.take());
    let err = drain(child.stderr.take());
    let deadline = Instant::now() + limit;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Ok(s),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                // never join the readers here: a grandchild holding the pipes open would block
                // the trial past its limit. They end by themselves when the pipes close.
                drop((out, err));
                return Err(format!("did not finish in {}s", limit.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => break Err(e),
        }
    };
    let status = status.map_err(|e| format!("could not be waited for: {e}"))?;
    // a grandchild that kept a pipe open must not hold the trial: wait a moment, then go on
    let text = |h: std::sync::mpsc::Receiver<Vec<u8>>| {
        String::from_utf8_lossy(&h.recv_timeout(Duration::from_secs(2)).unwrap_or_default())
            .into_owned()
    };
    Ok(RunOutput {
        exit: status.code(),
        stdout: text(out),
        stderr: text(err),
    })
}

// ------------------------------------------------------------------------ normalizing

/// What does not count as a difference between two cig builds' dry runs: the builds' own version
/// strings, run id lines, timestamps and the scratch directory.
///
/// Confirmed against cig 1.20.0 (`tests/golden/cig-1.20.0-dry-run`): its dry run prints none of
/// these, and identical scripts gave identical bytes on repeated runs. They are masked anyway
/// because a build that adds a banner or a run id line would otherwise make every script differ.
pub fn normalize(text: &str, versions: &[String], scratch: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for raw in text.replace("\r\n", "\n").lines() {
        let lower = raw.trim_start().to_ascii_lowercase();
        let run_id = ["run id", "run-id", "run_id", "runid"]
            .iter()
            .any(|p| lower.starts_with(p));
        if run_id {
            continue;
        }
        let mut line = raw.trim_end().to_string();
        if !scratch.is_empty() {
            line = line.replace(scratch, "<scratch>");
        }
        for v in versions {
            line = mask_word(&line, v, "<version>");
        }
        out.push(mask_timestamps(&line));
    }
    while out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
    }
    out.join("\n")
}

fn mask_word(line: &str, needle: &str, with: &str) -> String {
    if needle.is_empty() {
        return line.to_string();
    }
    let mut out = String::new();
    let mut rest = line;
    while let Some(i) = rest.find(needle) {
        let before = rest[..i].chars().next_back();
        let after_s = &rest[i + needle.len()..];
        let mut after = after_s.chars();
        let a = after.next();
        let dotted = a == Some('.') && after.next().is_some_and(|c| c.is_ascii_digit());
        let ok = !before.is_some_and(|c| c.is_ascii_digit() || c == '.')
            && !a.is_some_and(|c| c.is_ascii_digit())
            && !dotted;
        out.push_str(&rest[..i]);
        out.push_str(if ok { with } else { needle });
        rest = after_s;
    }
    out.push_str(rest);
    out
}

fn digits(b: &[u8], at: usize, n: usize) -> bool {
    b.len() >= at + n && b[at..at + n].iter().all(u8::is_ascii_digit)
}

/// Length of an ISO 8601 date-time at the start of `b`, or 0.
fn timestamp_len(b: &[u8]) -> usize {
    if !(digits(b, 0, 4)
        && b.get(4) == Some(&b'-')
        && digits(b, 5, 2)
        && b.get(7) == Some(&b'-')
        && digits(b, 8, 2)
        && matches!(b.get(10), Some(b'T') | Some(b' '))
        && digits(b, 11, 2)
        && b.get(13) == Some(&b':')
        && digits(b, 14, 2)
        && b.get(16) == Some(&b':')
        && digits(b, 17, 2))
    {
        return 0;
    }
    let mut n = 19;
    if b.get(n) == Some(&b'.') && digits(b, n + 1, 1) {
        n += 1;
        while digits(b, n, 1) {
            n += 1;
        }
    }
    match b.get(n) {
        Some(b'Z') => n += 1,
        Some(b'+') | Some(b'-') if digits(b, n + 1, 2) => {
            if b.get(n + 3) == Some(&b':') && digits(b, n + 4, 2) {
                n += 6;
            } else if digits(b, n + 3, 2) {
                n += 5;
            }
        }
        _ => {}
    }
    n
}

fn mask_timestamps(line: &str) -> String {
    let b = line.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < line.len() {
        if b[i].is_ascii_digit() {
            let n = timestamp_len(&b[i..]);
            if n > 0 {
                out.push_str("<time>");
                i += n;
                continue;
            }
        }
        let ch = line[i..].chars().next().unwrap_or(' ');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

// ------------------------------------------------------------------------------- diff

/// A unified-style diff of two texts, two lines of context, empty when they are equal.
pub fn unified_diff(old_label: &str, new_label: &str, a: &str, b: &str) -> String {
    let x: Vec<&str> = a.lines().collect();
    let y: Vec<&str> = b.lines().collect();
    if x == y {
        return String::new();
    }
    let mut out = format!("--- {old_label}\n+++ {new_label}\n");
    if x.len().saturating_mul(y.len()) > 1_000_000 {
        let at = x
            .iter()
            .zip(&y)
            .position(|(p, q)| p != q)
            .unwrap_or(x.len().min(y.len()));
        out.push_str(&format!(
            "(too long to diff line by line: {} and {} lines; they first differ at line {})\n",
            x.len(),
            y.len(),
            at + 1
        ));
        return out;
    }
    // longest common subsequence
    let (n, m) = (x.len(), y.len());
    let mut t = vec![0u32; (n + 1) * (m + 1)];
    let w = m + 1;
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            t[i * w + j] = if x[i] == y[j] {
                t[(i + 1) * w + j + 1] + 1
            } else {
                t[(i + 1) * w + j].max(t[i * w + j + 1])
            };
        }
    }
    let mut ops: Vec<(char, &str)> = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if x[i] == y[j] {
            ops.push((' ', x[i]));
            i += 1;
            j += 1;
        } else if t[(i + 1) * w + j] >= t[i * w + j + 1] {
            ops.push(('-', x[i]));
            i += 1;
        } else {
            ops.push(('+', y[j]));
            j += 1;
        }
    }
    ops.extend(x[i..].iter().map(|l| ('-', *l)));
    ops.extend(y[j..].iter().map(|l| ('+', *l)));
    const CTX: usize = 2;
    let shown: Vec<bool> = (0..ops.len())
        .map(|k| {
            let lo = k.saturating_sub(CTX);
            let hi = (k + CTX + 1).min(ops.len());
            ops[lo..hi].iter().any(|(c, _)| *c != ' ')
        })
        .collect();
    let mut gap = false;
    for (k, (c, l)) in ops.iter().enumerate() {
        if shown[k] {
            if gap {
                out.push_str("@@\n");
                gap = false;
            }
            out.push_str(&format!("{c}{l}\n"));
        } else {
            gap = true;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use cigi_shim::trusted_for;

    const HELLO: &str = include_str!("../../tests/golden/cig-1.20.0-dry-run/hello.txt");
    const USAGE: &str = include_str!("../../tests/golden/cig-1.20.0-dry-run/usage-error.txt");
    const BURN: &str = include_str!("../../tests/golden/cig-1.20.0-dry-run/burn.txt");

    fn versions() -> Vec<String> {
        vec!["1.20.0".into(), "1.21.0".into()]
    }

    /// What a real cig 1.20.0 dry run varies: nothing the normalizer has to change. The recorded
    /// outputs of three scripts, run twice each, were byte-identical, and carry none of the things
    /// the normalizer masks; so normalizing leaves them as they are.
    #[test]
    fn real_cig_1_20_0_dry_run_needs_no_masking() {
        for g in [HELLO, USAGE, BURN] {
            let body = g.lines().skip(1).collect::<Vec<_>>().join("\n");
            let (stdout, stderr) = body.split_once("--- stderr\n").unwrap_or((&body, ""));
            let stdout = stdout.trim_start_matches("--- stdout\n");
            for part in [stdout, stderr] {
                assert_eq!(
                    normalize(part, &versions(), "/nonexistent"),
                    part.trim_end().replace("\r\n", "\n")
                );
            }
        }
        // and the plan is on stderr: a trial that read only stdout would miss a changed plan
        assert!(BURN.contains("dry-run plan: 1 op"));
        assert!(!BURN
            .split("--- stderr")
            .next()
            .unwrap_or("")
            .contains("plan"));
    }

    #[test]
    fn masks_versions_run_ids_times_and_the_scratch_path() {
        let text = "cig 1.20.0 starting\nRun id: 1a2b3c\nat 2026-11-12T10:03:11Z and 2026-11-12 10:03:11.250+01:00\n\
                    wrote /tmp/x/cigi-trial-1/home/a\nversion 11.20.05 stays, 1.20.0.1 stays, v1.21.0 stays\n";
        let n = normalize(text, &versions(), "/tmp/x/cigi-trial-1");
        assert_eq!(
            n,
            "cig <version> starting\nat <time> and <time>\nwrote <scratch>/home/a\n\
             version 11.20.05 stays, 1.20.0.1 stays, v<version> stays"
        );
        // the same line from the two builds is the same after masking
        assert_eq!(
            normalize("cig 1.20.0 plan", &versions(), ""),
            normalize("cig 1.21.0 plan", &versions(), "")
        );
    }

    #[test]
    fn a_real_difference_survives_masking() {
        assert_ne!(
            normalize("plan: 1 op\n", &versions(), ""),
            normalize("plan: 2 ops\n", &versions(), "")
        );
    }

    #[test]
    fn diff_shows_changes_with_context_and_is_empty_when_equal() {
        assert_eq!(unified_diff("a", "b", "x\ny", "x\ny"), "");
        let d = unified_diff(
            "old",
            "new",
            "1\n2\n3\n4\n5\n6\n7\n8\n9",
            "1\n2\n3\n4\nFIVE\n6\n7\n8\n9",
        );
        assert!(d.starts_with("--- old\n+++ new\n"), "{d}");
        assert!(d.contains("-5\n+FIVE\n"), "{d}");
        assert!(!d.contains(" 1\n"), "context is two lines only: {d}");
    }

    #[cfg(unix)]
    fn fake_cig(dir: &Path, name: &str, body: &str) -> Resolved {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join(name);
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        Resolved {
            tool: "cig".into(),
            carton: "0.1.0".into(),
            version: Version::parse("1.20.0").unwrap(),
            path: p,
            cig: None,
        }
    }

    /// A fixture of another tool: a shell script that prints what a real one prints.
    #[cfg(unix)]
    fn fake_tool(dir: &Path, tool: &str, name: &str, version: &str, body: &str) -> Resolved {
        let mut r = fake_cig(dir, name, body);
        r.tool = tool.into();
        r.version = Version::parse(version).unwrap();
        r
    }

    #[cfg(unix)]
    fn project(d: &Path, files: &[(&str, &str)]) -> ProjectCtx {
        let proj = d.join("p");
        std::fs::create_dir_all(&proj).unwrap();
        for (f, body) in files {
            std::fs::write(proj.join(f), body).unwrap();
        }
        ProjectCtx {
            root: proj,
            ..ProjectCtx::default()
        }
    }

    #[cfg(unix)]
    #[test]
    fn cigo_trial_runs_check_and_the_dry_run_of_each_chain_and_finds_the_one_that_differs() {
        let d = tempfile::tempdir().unwrap();
        let ctx = project(
            d.path(),
            &[(
                "cig-tasks.json",
                r#"{"project":"p","tasks":{"build":{},"-flag":{},"has space":{},"ci":{}}}"#,
            )],
        );
        // the new carton's cig, which the new cigo must find first on PATH
        std::fs::create_dir_all(d.path().join("newcig")).unwrap();
        let newcig = fake_tool(&d.path().join("newcig"), "cig", "cig", "1.21.0", "echo cig");
        let old = fake_tool(
            d.path(),
            "cigo",
            "cigo-old",
            "1.17.0",
            r#"if [ "$1" = check ]; then echo "check: ok"; exit 0; fi
for a; do l=$a; done
echo "plan $l: 2 tasks (cigo 1.17.0)""#,
        );
        let mut new = fake_tool(
            d.path(),
            "cigo",
            "cigo-new",
            "1.18.0",
            r#"if [ "$1" = check ]; then echo "check: ok"; exit 0; fi
for a; do l=$a; done
case "$l" in
  build) echo "plan build: 3 tasks (cigo 1.18.0)"; echo "args: $*"; echo "cig=$(command -v cig)" ;;
  *) echo "plan $l: 2 tasks (cigo 1.18.0)" ;;
esac"#,
        );
        new.cig = Some(newcig.path.clone());
        let o = CigoTrial {
            trust: trusted_for(d.path()),
        }
        .run(&old, &new, &ctx, Duration::from_secs(30));
        // check, `ci` and `build`: a name with a leading dash or a space is never a chain
        assert_eq!((o.compared, o.same), (3, 2), "{o:?}");
        assert_eq!(o.different.len(), 1, "{o:?}");
        let d0 = &o.different[0];
        assert_eq!(d0.script, "cigo --dry-run run build");
        assert!(d0.diff.contains("+plan build: 3 tasks"), "{}", d0.diff);
        assert!(
            d0.diff
                .contains("+args: --dry-run --plain --no-color --no-doctor run build"),
            "{}",
            d0.diff
        );
        assert!(
            d0.diff.contains(&format!("+cig={}", newcig.path.display())),
            "the carton's cig comes first on PATH: {}",
            d0.diff
        );
    }

    #[cfg(unix)]
    #[test]
    fn cigo_trial_reports_a_changed_check_and_skips_a_project_with_no_manifest() {
        let d = tempfile::tempdir().unwrap();
        let ctx = project(d.path(), &[("cig-tasks.json", "{}")]);
        let old = fake_tool(d.path(), "cigo", "o", "1.17.0", r#"echo "fine""#);
        let new = fake_tool(
            d.path(),
            "cigo",
            "n",
            "1.18.0",
            r#"if [ "$1" = check ]; then echo "E999: unknown key" >&2; exit 1; fi
echo "fine""#,
        );
        let trial = CigoTrial {
            trust: trusted_for(d.path()),
        };
        let o = trial.run(&old, &new, &ctx, Duration::from_secs(30));
        assert_eq!((o.compared, o.same, o.different.len()), (2, 1, 1), "{o:?}");
        assert_eq!(o.different[0].script, "cigo check");
        assert!(o.different[0].what.contains("exit 0 -> 1"), "{o:?}");
        assert!(o.different[0].diff.contains("+E999: unknown key"));

        let none = ProjectCtx {
            root: d.path().join("empty"),
            ..ProjectCtx::default()
        };
        let o = trial.run(&old, &new, &none, Duration::from_secs(30));
        assert_eq!(o.compared, 0);
        assert_eq!(o.skipped, ["cigo: no cig-tasks.json in the project"]);
    }

    #[cfg(unix)]
    #[test]
    fn cigcompile_trial_tests_each_program_into_scratch_and_shows_the_difference() {
        let d = tempfile::tempdir().unwrap();
        let ctx = project(d.path(), &[("a.cig", "x\n"), ("b.cig", "x\n")]);
        let before: Vec<_> = std::fs::read_dir(&ctx.root)
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        let body = |extra: &str| {
            format!(
                r#"[ "$1" = test ] || exit 2
out=$2; [ "$2" = --out ] && out=$3
echo built > "$out" || exit 3
for a; do l=$a; done
echo "built $out"
case "$l" in
  ./b.cig) {extra} ;;
  *) echo "same     $l (dry run: the same plan, output and exit)" ;;
esac"#
            )
        };
        let old = fake_tool(
            d.path(),
            "cigcompile",
            "cc-old",
            "0.2.0",
            &body(r#"echo "same     ./b.cig (dry run: the same plan, output and exit)""#),
        );
        let new = fake_tool(
            d.path(),
            "cigcompile",
            "cc-new",
            "0.3.0",
            &body(r#"echo "DIFFERS  ./b.cig"; echo "  stdout differs"; exit 1"#),
        );
        let o = CigcompileTrial {
            trust: trusted_for(d.path()),
        }
        .run(&old, &new, &ctx, Duration::from_secs(30));
        assert_eq!((o.compared, o.same), (2, 1), "{o:?}");
        assert_eq!(o.different.len(), 1);
        assert_eq!(o.different[0].script, "b.cig (cigcompile test)");
        assert!(o.different[0].what.contains("exit 0 -> 1"));
        assert!(o.different[0].diff.contains("+DIFFERS  ./b.cig"));
        // the programs went to scratch: nothing was added to the project
        let after: Vec<_> = std::fs::read_dir(&ctx.root)
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(before.len(), after.len(), "{after:?}");
        // a project with no programs says so
        let empty = ProjectCtx {
            root: d.path().join("none"),
            ..ProjectCtx::default()
        };
        let o = CigcompileTrial {
            trust: trusted_for(d.path()),
        }
        .run(&old, &new, &empty, Duration::from_secs(30));
        assert_eq!(o.compared, 0);
        assert!(o.skipped[0].contains("no .cig programs"), "{o:?}");
    }

    #[cfg(unix)]
    #[test]
    fn cig_trial_compares_runs_and_finds_the_script_that_differs() {
        let d = tempfile::tempdir().unwrap();
        let proj = d.path().join("p");
        std::fs::create_dir_all(proj.join("sub")).unwrap();
        std::fs::create_dir_all(proj.join(".git")).unwrap();
        std::fs::create_dir_all(proj.join("node_modules")).unwrap();
        for f in [
            "a.cig",
            "sub/b.cig",
            ".git/c.cig",
            "node_modules/d.cig",
            "e.txt",
        ] {
            std::fs::write(proj.join(f), "x\n").unwrap();
        }
        let (found, truncated) = find_scripts(&proj, trusted_for(&proj));
        assert!(!truncated);
        let names: Vec<String> = found.iter().map(|p| rel_label(&proj, p)).collect();
        assert_eq!(names.len(), 2, "{names:?}");
        assert!(names.contains(&"a.cig".to_string()) && names.contains(&"sub/b.cig".to_string()));

        let old = fake_cig(
            d.path(),
            "old",
            "for a; do l=$a; done; echo \"plan for $l by cig 1.20.0\"; echo ok >&2",
        );
        let mut new = fake_cig(
            d.path(),
            "new",
            "for a; do l=$a; done; case \"$l\" in *b.cig) echo changed;; *) echo \"plan for $l by cig 1.21.0\";; esac; echo ok >&2",
        );
        new.version = Version::parse("1.21.0").unwrap();
        let ctx = ProjectCtx {
            root: proj.clone(),
            ..ProjectCtx::default()
        };
        let o = CigTrial {
            trust: trusted_for(d.path()),
        }
        .run(&old, &new, &ctx, Duration::from_secs(30));
        assert_eq!((o.compared, o.same), (2, 1), "{o:?}");
        assert_eq!(o.different.len(), 1);
        assert_eq!(o.different[0].script, "sub/b.cig");
        assert!(
            o.different[0].diff.contains("+changed"),
            "{}",
            o.different[0].diff
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_script_that_fails_the_same_way_on_both_is_the_same_and_a_hang_is_skipped() {
        let d = tempfile::tempdir().unwrap();
        let proj = d.path().join("p");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("a.cig"), "x\n").unwrap();
        let ctx = ProjectCtx {
            root: proj,
            ..ProjectCtx::default()
        };
        let same = |n: &str| fake_cig(d.path(), n, "echo boom >&2; exit 2");
        let o = CigTrial {
            trust: trusted_for(d.path()),
        }
        .run(&same("o1"), &same("n1"), &ctx, Duration::from_secs(30));
        assert_eq!((o.compared, o.same, o.different.len()), (1, 1, 0));
        // a budget already spent runs nothing and says so
        let o = CigTrial {
            trust: trusted_for(d.path()),
        }
        .run(&same("o2"), &same("n2"), &ctx, Duration::ZERO);
        assert_eq!(o.compared, 0);
        assert!(o.skipped.iter().any(|s| s.contains("budget")), "{o:?}");
    }

    #[test]
    fn the_three_trials_are_registered() {
        let r = registered(trusted_for(Path::new("/")));
        let tools: Vec<&str> = r.iter().map(|t| t.tool()).collect();
        assert_eq!(tools, ["cig", "cigo", "cigcompile"]);
    }
}
