// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `bench` (#35, as far as it can be built today): the installer's own budgets (#20, #32)
//! measured on this machine, locally, and compared with `bench/bench.toml`.
//!
//! Nothing here touches the real home beyond reading it. The shim and the switch are measured
//! in a scratch home made for the run and removed after it; the marry-up check reads the cached
//! manifest and never fetches; sizes are `stat`s. `--from <mirror>` installs into a scratch home
//! with its own `HOME`, so no profile and no real file is written. Nothing is sent anywhere.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cigi_home::state::CartonRecord;
use cigi_home::{
    installed, now_rfc3339, read_receipt, FlipIntent, Home, Journal, ShimIndex, State, Txn,
};
use serde_json::{json, Value};

use crate::cx::{Cx, Globals};

/// The budgets, embedded.
pub const BUDGETS_TOML: &str = include_str!("../../../../bench/bench.toml");

/// A budget a measurement is compared with.
#[derive(Debug, Clone)]
pub struct Budget {
    pub id: String,
    pub what: String,
    pub limit: f64,
    pub unit: String,
    pub how: String,
    pub source: String,
}

/// The budgets of `bench/bench.toml`. A malformed file reads as none (a build defect the
/// tests catch, never a panic for the person running the command).
pub fn budgets() -> Vec<Budget> {
    let Ok(doc) = BUDGETS_TOML.parse::<toml_edit::DocumentMut>() else {
        return Vec::new();
    };
    let Some(items) = doc.get("budget").and_then(|b| b.as_array_of_tables()) else {
        return Vec::new();
    };
    let text = |t: &toml_edit::Table, k: &str| {
        t.get(k)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };
    items
        .iter()
        .map(|t| Budget {
            id: text(t, "id"),
            what: text(t, "what"),
            limit: t.get("limit").and_then(|v| v.as_float()).unwrap_or(0.0),
            unit: text(t, "unit"),
            how: text(t, "how"),
            source: text(t, "source"),
        })
        .collect()
}

/// What a row says about its budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok,
    Over,
    /// Measured, but this build or this machine cannot judge it.
    Info,
    NotMeasured,
}

impl Status {
    pub fn word(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Over => "over",
            Status::Info => "info",
            Status::NotMeasured => "not measured",
        }
    }
}

/// One line of the report.
#[derive(Debug, Clone)]
pub struct Row {
    pub budget: Budget,
    pub median: Option<f64>,
    pub p95: Option<f64>,
    pub runs: usize,
    pub status: Status,
    pub note: String,
}

impl Row {
    fn measured(b: &Budget, samples: &mut [f64], note: &str) -> Row {
        let (median, p95) = stats(samples);
        let status = if median <= b.limit {
            Status::Ok
        } else {
            Status::Over
        };
        Row {
            budget: b.clone(),
            median: Some(median),
            p95: Some(p95),
            runs: samples.len(),
            status,
            note: note.to_string(),
        }
    }

    fn single(b: &Budget, value: f64, status: Status, note: &str) -> Row {
        Row {
            budget: b.clone(),
            median: Some(value),
            p95: None,
            runs: 1,
            status,
            note: note.to_string(),
        }
    }

    fn not_measured(b: &Budget, why: impl Into<String>) -> Row {
        Row {
            budget: b.clone(),
            median: None,
            p95: None,
            runs: 0,
            status: Status::NotMeasured,
            note: why.into(),
        }
    }
}

/// `(median, p95)` by nearest rank. Sorts the samples.
pub fn stats(samples: &mut [f64]) -> (f64, f64) {
    if samples.is_empty() {
        return (0.0, 0.0);
    }
    samples.sort_by(|a, b| a.total_cmp(b));
    let n = samples.len();
    let rank = |q: f64| ((q * n as f64).ceil() as usize).clamp(1, n) - 1;
    (samples[rank(0.5)], samples[rank(0.95)])
}

/// What a run was asked to do.
#[derive(Debug, Clone)]
pub struct Options {
    /// Shim dispatches measured; the heavier measurements take a fifth of it, at least five.
    pub runs: usize,
    /// Where the scratch home goes (default: the system's temporary directory).
    pub scratch: Option<PathBuf>,
    /// A local mirror to time installs from.
    pub from: Option<PathBuf>,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            runs: 100,
            scratch: None,
            from: None,
        }
    }
}

impl Options {
    fn heavy(&self) -> usize {
        (self.runs / 5).max(5)
    }
    fn light(&self) -> usize {
        (self.runs / 10).max(5)
    }
}

/// A scratch directory, removed when it goes out of scope. It is made with `create_dir`, so a
/// directory that already exists is never taken over, and only that one is ever removed.
struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(base: Option<&Path>) -> Result<Scratch, String> {
        let base = base
            .map(Path::to_path_buf)
            .unwrap_or_else(std::env::temp_dir);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let root = base.join(format!("ciginstaller-bench-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&base)
            .and_then(|()| std::fs::create_dir(&root))
            .map_err(|e| format!("cannot make a scratch directory in {}: {e}", base.display()))?;
        Ok(Scratch { root })
    }
    fn home(&self) -> PathBuf {
        self.root.join("home")
    }
    fn user(&self) -> PathBuf {
        self.root.join("user")
    }
    fn project(&self) -> PathBuf {
        self.root.join("project")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn exe() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|e| format!("cannot find this program: {e}"))
}

fn shim_file(exe: &Path) -> PathBuf {
    exe.with_file_name(format!("cigi-shim{}", std::env::consts::EXE_SUFFIX))
}

/// `Command::output`, tried again while exec fails with "text file busy": a file written a
/// moment ago can still be open in a child another thread forked.
fn output_retrying(c: &mut Command) -> std::io::Result<std::process::Output> {
    let mut tries = 0;
    loop {
        match c.output() {
            Err(e) if e.raw_os_error() == Some(26) && tries < 200 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(5));
            }
            r => return r,
        }
    }
}

/// The shim's own time, in nanoseconds, from process start to `exec` (`CIGI_SHIM_TIMING=1`),
/// over a scratch home whose index points `cig` at this program. The real shim file is copied,
/// so what is timed is the shipped code.
fn shim_samples(scratch: &Scratch, runs: usize) -> Result<Vec<f64>, String> {
    let exe = exe()?;
    let src = shim_file(&exe);
    if !src.is_file() {
        return Err(format!("cigi-shim is not next to {}", exe.display()));
    }
    let (home, user, project) = (scratch.home(), scratch.user(), scratch.project());
    for d in [
        home.join("bin"),
        home.join("installer"),
        user.clone(),
        project.clone(),
    ] {
        std::fs::create_dir_all(&d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let shim = home
        .join("bin")
        .join(format!("cig{}", std::env::consts::EXE_SUFFIX));
    std::fs::copy(&src, &shim).map_err(|e| format!("cannot copy the shim: {e}"))?;
    let index = format!(
        "cigi-index 1\nmachine\t0.0.0\ncarton\t0.0.0\tcig\t{}\nlegacy-pin-files\t.cig-version\n",
        exe.display()
    );
    std::fs::write(home.join("installer").join("shim-index"), index)
        .map_err(|e| format!("cannot write the scratch index: {e}"))?;
    let mut samples = Vec::with_capacity(runs);
    for i in 0..runs + 3 {
        let mut c = Command::new(&shim);
        c.env_clear()
            .env("HOME", &user)
            .env("CIGSCRIPT_HOME", &home)
            .env("CIGI_SHIM_TIMING", "1")
            .current_dir(&project)
            .arg("--version")
            .stdin(Stdio::null());
        let o = output_retrying(&mut c).map_err(|e| format!("cannot run the shim: {e}"))?;
        let err = String::from_utf8_lossy(&o.stderr);
        let ns = err
            .lines()
            .find_map(|l| l.strip_prefix("cigi-shim-timing-ns: "))
            .and_then(|n| n.trim().parse::<f64>().ok())
            .ok_or_else(|| "the shim printed no timing line".to_string())?;
        if i >= 3 {
            samples.push(ns);
        }
    }
    Ok(samples)
}

/// The commit of a switch, in a scratch home: the state written through the journal, the index
/// flipped atomically, the transaction committed. Alternates between two cartons.
fn switch_samples(scratch: &Scratch, runs: usize) -> Result<Vec<f64>, String> {
    let exe = exe()?;
    let home = Home::at(scratch.home());
    home.ensure_layout().map_err(|e| e.to_string())?;
    let lock = home.lock().map_err(|e| e.to_string())?;
    let mut state = State::default();
    let record = |tool_version: &str| CartonRecord {
        installed_at: now_rfc3339(),
        tools: [("cig".to_string(), tool_version.to_string())]
            .into_iter()
            .collect(),
        reason: "bench".to_string(),
        last_used: None,
        partial: false,
    };
    state.cartons.insert("0.0.1".into(), record("1.0.0"));
    state.cartons.insert("0.0.2".into(), record("1.0.1"));
    let mut samples = Vec::with_capacity(runs);
    for i in 0..runs + 1 {
        let to = if i % 2 == 0 { "0.0.1" } else { "0.0.2" };
        let t = Instant::now();
        let mut txn = Txn::begin(&home, "bench-switch", &lock).map_err(|e| e.to_string())?;
        state.machine_carton = Some(to.to_string());
        state.save_in(&mut txn).map_err(|e| e.to_string())?;
        let mut index = ShimIndex {
            machine: Some(to.to_string()),
            ..ShimIndex::default()
        };
        for c in ["0.0.1", "0.0.2"] {
            index
                .cartons
                .insert(c.to_string(), [("cig".to_string(), exe.clone())].into());
        }
        index.check().map_err(|e| e.to_string())?;
        txn.flip(FlipIntent {
            index: index.render(),
        })
        .map_err(|e| e.to_string())?;
        txn.commit().map_err(|e| e.to_string())?;
        if i > 0 {
            samples.push(t.elapsed().as_secs_f64() * 1000.0);
        }
    }
    Ok(samples)
}

/// `check` against the cached manifest, in this process: opening the home, reading the cached
/// metadata and running the matrix. Never the network (`offline`). The process's own start is
/// not in the number; the first-line rows carry that.
fn check_samples(cx: &Cx, runs: usize) -> Result<Vec<f64>, String> {
    let globals = Globals {
        offline: true,
        ..cx.globals.clone()
    };
    let mut samples = Vec::with_capacity(runs);
    for _ in 0..runs {
        let t = Instant::now();
        let mut fresh =
            Cx::new(cx.ui.duplicate(), globals.clone(), None).map_err(|e| e.message.clone())?;
        crate::cmd::check::run(&mut fresh, None).map_err(|e| e.message.clone())?;
        samples.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    Ok(samples)
}

/// Spawn this program with `args` and time the first byte on stdout or stderr.
fn first_byte(exe: &Path, home: &Path, args: &[&str]) -> Result<f64, String> {
    use std::io::Read;
    let start = Instant::now();
    let mut c = Command::new(exe);
    c.arg("--home")
        .arg(home)
        .args(["--plain", "--no-color"])
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = c
        .spawn()
        .map_err(|e| format!("cannot run {}: {e}", exe.display()))?;
    let (tx, rx) = std::sync::mpsc::channel();
    let mut readers = Vec::new();
    if let Some(mut o) = child.stdout.take() {
        let tx = tx.clone();
        readers.push(std::thread::spawn(move || {
            let mut b = [0u8; 1];
            if o.read(&mut b).unwrap_or(0) > 0 {
                let _ = tx.send(Instant::now());
            }
            let _ = std::io::copy(&mut o, &mut std::io::sink());
        }));
    }
    if let Some(mut e) = child.stderr.take() {
        readers.push(std::thread::spawn(move || {
            let mut b = [0u8; 1];
            if e.read(&mut b).unwrap_or(0) > 0 {
                let _ = tx.send(Instant::now());
            }
            let _ = std::io::copy(&mut e, &mut std::io::sink());
        }));
    }
    let first = rx.recv_timeout(Duration::from_secs(10)).ok();
    let _ = child.wait();
    for r in readers {
        let _ = r.join();
    }
    first
        .map(|t| t.duration_since(start).as_secs_f64() * 1000.0)
        .ok_or_else(|| "it wrote nothing".to_string())
}

fn first_line_samples(cx: &Cx, args: &[&str], runs: usize) -> Result<Vec<f64>, String> {
    let exe = exe()?;
    let home = cx.home.root();
    (0..runs).map(|_| first_byte(&exe, &home, args)).collect()
}

/// Distinct bytes of every cigo version held, against the biggest one alone.
/// `(versions held, ratio)`; the ratio is `None` with fewer than two.
pub fn disk_ratio(cx: &Cx) -> (usize, Option<f64>) {
    let mut seen = std::collections::BTreeMap::new();
    let mut biggest = 0u64;
    let mut held = 0usize;
    for (tool, v) in installed(&cx.home) {
        if tool != "cigo" {
            continue;
        }
        let Ok(r) = read_receipt(&cx.home, &tool, &v) else {
            continue;
        };
        held += 1;
        biggest = biggest.max(r.files.iter().map(|f| f.bytes).sum());
        for f in &r.files {
            seen.insert(f.sha256.clone(), f.bytes);
        }
    }
    if held < 2 || biggest == 0 {
        return (held, None);
    }
    let all: u64 = seen.values().sum();
    (held, Some(all as f64 / biggest as f64))
}

/// Time `install pack dev` and `install carton` from a local mirror into a scratch home with its
/// own `HOME`: no profile and no real file is written. One run each.
fn install_samples(scratch: &Scratch, mirror: &Path, what: &[&str]) -> Result<f64, String> {
    let exe = exe()?;
    let (home, user) = (scratch.home(), scratch.user());
    std::fs::create_dir_all(&user).map_err(|e| e.to_string())?;
    let t = Instant::now();
    let o = output_retrying(
        Command::new(&exe)
            .env("HOME", &user)
            .env("CIGSCRIPT_HOME", &home)
            .env_remove("CIGI_MIRROR")
            .args(["--yes", "--plain", "--no-color", "install"])
            .args(what)
            .arg("--no-path")
            .arg("--from")
            .arg(mirror)
            .stdin(Stdio::null()),
    )
    .map_err(|e| e.to_string())?;
    let took = t.elapsed().as_secs_f64() * 1000.0;
    if o.status.success() {
        Ok(took)
    } else {
        let err = String::from_utf8_lossy(&o.stderr);
        let first = err.lines().next().unwrap_or("it failed");
        Err(format!("the install failed: {first}"))
    }
}

/// Every measurement, in the order of `bench/bench.toml`.
pub fn run_all(cx: &Cx, opts: &Options) -> Vec<Row> {
    let all = budgets();
    let mut rows = Vec::new();
    // one scratch home serves the shim, the switch and the install
    let scratch = Scratch::new(opts.scratch.as_deref());
    for b in &all {
        let row = match b.id.as_str() {
            "shim_dispatch" => match &scratch {
                Ok(s) => match shim_samples(s, opts.runs) {
                    Ok(ns) => {
                        let mut ms: Vec<f64> = ns.iter().map(|n| n / 1.0e6).collect();
                        Row::measured(b, &mut ms, "the shim's own time to exec")
                    }
                    Err(e) => Row::not_measured(b, e),
                },
                Err(e) => Row::not_measured(b, e.clone()),
            },
            "check" => match check_samples(cx, opts.heavy()) {
                Ok(mut ms) => {
                    Row::measured(b, &mut ms, "in this process, from the cached manifest")
                }
                Err(e) => Row::not_measured(b, format!("no cached manifest to check ({e})")),
            },
            "switch" => match &scratch {
                Ok(s) => match switch_samples(s, opts.heavy()) {
                    Ok(mut ms) => Row::measured(
                        b,
                        &mut ms,
                        "state, index flip and commit, in a scratch home",
                    ),
                    Err(e) => Row::not_measured(b, e),
                },
                Err(e) => Row::not_measured(b, e.clone()),
            },
            "disk_five_cigo" => match disk_ratio(cx) {
                (_, None) => Row::not_measured(b, "needs two or more cigo versions held"),
                (n, Some(r)) if r > b.limit => {
                    Row::single(b, r, Status::Over, &format!("{n} cigo versions held"))
                }
                (n, Some(r)) if n >= 5 => {
                    Row::single(b, r, Status::Ok, &format!("{n} cigo versions held"))
                }
                (n, Some(r)) => Row::single(
                    b,
                    r,
                    Status::Info,
                    &format!("{n} cigo versions held; the budget is for five"),
                ),
            },
            "installer_bytes" | "shim_bytes" => {
                let path = exe().map(|e| {
                    if b.id == "shim_bytes" {
                        shim_file(&e)
                    } else {
                        e
                    }
                });
                match path.and_then(|p| {
                    std::fs::metadata(&p)
                        .map(|m| m.len())
                        .map_err(|e| format!("{}: {e}", p.display()))
                }) {
                    Ok(n) if cfg!(debug_assertions) => Row::single(
                        b,
                        n as f64,
                        Status::Info,
                        "a debug build; the budget is for the release build",
                    ),
                    Ok(n) => Row::single(
                        b,
                        n as f64,
                        if n as f64 <= b.limit {
                            Status::Ok
                        } else {
                            Status::Over
                        },
                        "",
                    ),
                    Err(e) => Row::not_measured(b, e),
                }
            }
            "first_line_status" | "first_line_list" => {
                let args: &[&str] = if b.id == "first_line_status" {
                    &[]
                } else {
                    &["list"]
                };
                match first_line_samples(cx, args, opts.light()) {
                    Ok(mut ms) => Row::measured(b, &mut ms, "process start to first byte"),
                    Err(e) => Row::not_measured(b, e),
                }
            }
            "install_pack_dev" | "install_carton" => match (&opts.from, &scratch) {
                (None, _) => Row::not_measured(b, "needs a local mirror: --from <dir>"),
                (_, Err(e)) => Row::not_measured(b, e.clone()),
                (Some(m), Ok(s)) => {
                    // each install gets a clean home inside the scratch directory
                    let what: &[&str] = if b.id == "install_pack_dev" {
                        &["pack", "dev"]
                    } else {
                        &["carton"]
                    };
                    let inner = Scratch::new(Some(&s.root));
                    match inner.and_then(|i| install_samples(&i, m, what)) {
                        Ok(ms) => Row::single(
                            b,
                            ms,
                            if ms <= b.limit {
                                Status::Ok
                            } else {
                                Status::Over
                            },
                            "one install from a local mirror, no network",
                        ),
                        Err(e) => Row::not_measured(b, e),
                    }
                }
            },
            // measured by a test, not here
            _ => continue,
        };
        rows.push(row);
    }
    rows
}

fn r2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// A value in its unit, for people.
pub fn show(value: f64, unit: &str) -> String {
    match unit {
        "bytes" if value >= 1_000_000.0 => format!("{:.2} MB", value / 1_000_000.0),
        "bytes" => format!("{:.0} KB", value / 1_000.0),
        "x" => format!("{:.2}x", value),
        _ if value >= 1000.0 => format!("{:.2} s", value / 1000.0),
        _ => format!("{:.2} ms", value),
    }
}

fn find<'a>(rows: &'a [Row], id: &str) -> Option<&'a Row> {
    rows.iter().find(|r| r.budget.id == id)
}

/// The reserved `bench` row of `docs/JSON.md`, extended: `null` for what was not measured.
pub fn json(rows: &[Row], opts: &Options) -> Value {
    let med = |id: &str| find(rows, id).and_then(|r| r.median);
    let p95 = |id: &str| find(rows, id).and_then(|r| r.p95);
    let ms = |v: Option<f64>| v.map(r2);
    let ns = |v: Option<f64>| v.map(|v| (v * 1.0e6).round() as u64);
    let bytes = |id: &str| med(id).map(|v| v as u64);
    let over = rows.iter().filter(|r| r.status == Status::Over).count();
    json!({
        "shim_dispatch_ns": ns(med("shim_dispatch")),
        "check_ms": ms(med("check")),
        "switch_ms": ms(med("switch")),
        "shim_bytes": bytes("shim_bytes"),
        "installer_bytes": bytes("installer_bytes"),
        "shim_dispatch_p95_ns": ns(p95("shim_dispatch")),
        "check_p95_ms": ms(p95("check")),
        "switch_p95_ms": ms(p95("switch")),
        "disk_five_cigo_ratio": med("disk_five_cigo").map(r2),
        "first_line_status_ms": ms(med("first_line_status")),
        "first_line_list_ms": ms(med("first_line_list")),
        "install_pack_dev_ms": ms(med("install_pack_dev")),
        "install_carton_ms": ms(med("install_carton")),
        "build": if cfg!(debug_assertions) { "debug" } else { "release" },
        "runs": opts.runs,
        "over_budget": over,
        "budgets": rows.iter().map(|r| json!({
            "id": r.budget.id,
            "what": r.budget.what,
            "source": r.budget.source,
            "unit": r.budget.unit,
            "limit": r.budget.limit,
            "median": r.median.map(r2),
            "p95": r.p95.map(r2),
            "runs": r.runs,
            "status": r.status.word(),
            "note": r.note,
        })).collect::<Vec<_>>(),
        "not_measured_by_bench": budgets().iter()
            .filter(|b| b.how == "test")
            .map(|b| json!({"id": b.id, "what": b.what, "limit": b.limit, "unit": b.unit}))
            .collect::<Vec<_>>(),
        "not_built": NOT_BUILT.iter()
            .map(|(view, blocker)| json!({"view": view, "blocked_by": blocker}))
            .collect::<Vec<_>>(),
    })
}

/// The views of #35 that are not built, and the issue's own words for what they wait on.
pub const NOT_BUILT: &[(&str, &str)] = &[
    (
        "the family view: every installed product's suite in one report",
        "wraps `cig bench --family`: depends on otmof-ops/cig-script#179 (the tool), open",
    ),
    (
        "cartons against cartons (--carton, --with, --matrix, --product, --only)",
        "a suite for each product to run: depends on otmof-ops/cig-script#179 (the tool) and #30 (the resolver), both open",
    ),
    (
        "live: --watch",
        "reruns a product's suite when its checkout changes (each product's [bench].watch): depends on otmof-ops/cig-script#179 (the tool), open",
    ),
    (
        "the window's Bench view",
        "built from #33's components and the tokens (#31): depends on #33 (the window), open",
    ),
    (
        "the cross-product cases (cigo's hook on each cig, a compiled program against cig run, an update's trial on a 50-script project, the window's timings)",
        "each measures another product or the window: depends on otmof-ops/cig-script#179 (the tool) and #33",
    ),
    (
        "report it (the third choice after a regression)",
        "reporting goes through #29, open",
    ),
];

// ---- after an update: a local check, never telemetry ----

fn record_path(cx: &Cx) -> PathBuf {
    cx.home.installer().join("bench").join("last.json")
}

/// A sample taken after an update: the three timings the update can change.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub shim_ns: f64,
    pub check_ms: f64,
    pub switch_ms: f64,
}

/// The size of a change that is shown: more than a fifth, and more than the noise floor.
fn regressions(old: &Sample, new: &Sample) -> Vec<String> {
    let mut out = Vec::new();
    let mut one = |name: &str, old: f64, new: f64, floor: f64, unit: &str, scale: f64| {
        if new > old * 1.2 && new - old > floor {
            out.push(format!(
                "{name} is {:.0}% slower here ({:.2} -> {:.2} {unit})",
                (new / old - 1.0) * 100.0,
                old / scale,
                new / scale
            ));
        }
    };
    one(
        "shim dispatch",
        old.shim_ns,
        new.shim_ns,
        100_000.0,
        "ms",
        1.0e6,
    );
    one("check", old.check_ms, new.check_ms, 5.0, "ms", 1.0);
    one("switch", old.switch_ms, new.switch_ms, 5.0, "ms", 1.0);
    out
}

fn read_sample(cx: &Cx) -> Option<Sample> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(record_path(cx)).ok()?).ok()?;
    Some(Sample {
        shim_ns: v.get("shim_dispatch_ns")?.as_f64()?,
        check_ms: v.get("check_ms")?.as_f64()?,
        switch_ms: v.get("switch_ms")?.as_f64()?,
    })
}

/// Written beside the other installer files, atomically; the numbers stay on this machine.
fn write_sample(cx: &Cx, s: &Sample) {
    let p = record_path(cx);
    let Some(dir) = p.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let body = json!({
        "format": 1,
        "at": now_rfc3339(),
        "carton": cx.state.machine_carton,
        "shim_dispatch_ns": s.shim_ns,
        "check_ms": s.check_ms,
        "switch_ms": s.switch_ms,
    });
    let text = format!(
        "{}\n",
        serde_json::to_string_pretty(&body).unwrap_or_default()
    );
    let tmp = dir.join(format!(".last.json.{}", std::process::id()));
    if std::fs::write(&tmp, text).is_ok() && std::fs::rename(&tmp, &p).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// Take the short sample (a few seconds): the shim, the check and the switch.
fn sample(cx: &Cx) -> Result<Sample, String> {
    let scratch = Scratch::new(None)?;
    let shim = shim_samples(&scratch, 30)?;
    let mut switch = switch_samples(&scratch, 5)?;
    let mut check = check_samples(cx, 5)?;
    let mut shim = shim;
    Ok(Sample {
        shim_ns: stats(&mut shim).0,
        check_ms: stats(&mut check).0,
        switch_ms: stats(&mut switch).0,
    })
}

/// `bench_after_update = on`: after an update that went well, a short local sample against the
/// last one this machine took. A regression is shown once (the new sample becomes the baseline).
/// Off by default; nothing is measured without the setting, and nothing leaves the machine.
pub fn after_update(cx: &Cx) {
    if cx.cfg.get("bench_after_update") != "on" {
        return;
    }
    let Ok(now) = sample(cx) else {
        return;
    };
    let before = read_sample(cx);
    let previous = State::load(&cx.home)
        .ok()
        .and_then(|s| s.previous.machine_carton);
    write_sample(cx, &now);
    let mut r = cigi_ui::Report::new(
        cigi_ui::Icon::Note,
        format!(
            "local check after the update: shim dispatch {:.2} ms, check {:.1} ms, switch {:.1} ms; nothing was sent anywhere",
            now.shim_ns / 1.0e6,
            now.check_ms,
            now.switch_ms
        ),
    );
    if let Some(old) = before {
        let slow = regressions(&old, &now);
        if !slow.is_empty() {
            r = r.detail(format!("since the last local sample, {}", slow.join(", ")));
            if let Some(p) = previous {
                r = r.detail(format!("ciginstaller rollback goes back to carton {p}"));
            }
            r = r.next("ciginstaller rollback");
        }
    }
    cx.ui.say(&r);
}

/// `E1866` for `bench --strict`, when anything is over its budget.
pub fn over_error(rows: &[Row]) -> Option<cigi_ui::InstallerError> {
    let over: Vec<String> = rows
        .iter()
        .filter(|r| r.status == Status::Over)
        .map(|r| {
            format!(
                "{} {} over {}",
                r.budget.what,
                show(r.median.unwrap_or(0.0), &r.budget.unit),
                show(r.budget.limit, &r.budget.unit)
            )
        })
        .collect();
    if over.is_empty() {
        return None;
    }
    Some(
        cigi_ui::InstallerError::new(
            "E1866",
            format!("{} budget(s) are over: {}", over.len(), over.join("; ")),
        )
        .with_fix("ciginstaller explain E1866"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_budgets_file_reads_and_has_the_numbers_of_issue_20() {
        let b = budgets();
        let get = |id: &str| b.iter().find(|x| x.id == id).map(|x| x.limit);
        assert_eq!(get("shim_dispatch"), Some(2.0));
        assert_eq!(get("check"), Some(200.0));
        assert_eq!(get("switch"), Some(100.0));
        assert_eq!(get("disk_five_cigo"), Some(1.5));
        assert_eq!(get("installer_bytes"), Some(10_000_000.0));
        assert_eq!(get("install_pack_dev"), Some(30_000.0));
        assert_eq!(get("install_carton"), Some(60_000.0));
        assert_eq!(get("update_check"), Some(1000.0));
        assert!(b.iter().all(|x| !x.what.is_empty() && !x.unit.is_empty()));
        let ids: std::collections::BTreeSet<_> = b.iter().map(|x| x.id.as_str()).collect();
        assert_eq!(ids.len(), b.len(), "a budget id is used twice");
    }

    #[test]
    fn stats_use_nearest_rank() {
        let mut v: Vec<f64> = (1..=100).map(f64::from).collect();
        v.reverse();
        assert_eq!(stats(&mut v), (50.0, 95.0));
        assert_eq!(stats(&mut [3.0]), (3.0, 3.0));
        assert_eq!(stats(&mut []), (0.0, 0.0));
    }

    #[test]
    fn a_regression_needs_a_fifth_and_more_than_the_noise() {
        let old = Sample {
            shim_ns: 240_000.0,
            check_ms: 54.0,
            switch_ms: 8.0,
        };
        let same = Sample {
            shim_ns: 250_000.0,
            check_ms: 56.0,
            switch_ms: 9.0,
        };
        assert!(regressions(&old, &same).is_empty());
        let slow = Sample {
            shim_ns: 240_000.0,
            check_ms: 70.0,
            switch_ms: 8.0,
        };
        let r = regressions(&old, &slow);
        assert_eq!(r.len(), 1, "{r:?}");
        assert!(
            r[0].contains("30% slower") && r[0].contains("54.00 -> 70.00"),
            "{r:?}"
        );
        // a fifth of nothing is not a regression
        let tiny_old = Sample {
            shim_ns: 100_000.0,
            check_ms: 1.0,
            switch_ms: 1.0,
        };
        let tiny_new = Sample {
            shim_ns: 150_000.0,
            check_ms: 2.0,
            switch_ms: 3.0,
        };
        assert!(regressions(&tiny_old, &tiny_new).is_empty());
    }

    #[test]
    fn strict_names_what_is_over() {
        let b = budgets();
        let mk = |id: &str, v: f64, st: Status| Row {
            budget: b.iter().find(|x| x.id == id).cloned().expect("a budget"),
            median: Some(v),
            p95: Some(v),
            runs: 1,
            status: st,
            note: String::new(),
        };
        assert!(over_error(&[mk("check", 50.0, Status::Ok)]).is_none());
        let e = over_error(&[
            mk("check", 50.0, Status::Ok),
            mk("switch", 250.0, Status::Over),
        ])
        .expect("an error");
        assert_eq!(e.code, "E1866");
        assert!(
            e.message
                .contains("an atomic switch to a new carton 250.00 ms over 100.00 ms"),
            "{}",
            e.message
        );
        assert!(!e.message.contains("cached metadata"));
    }

    #[test]
    fn the_scratch_directory_goes_away() {
        let root = {
            let s = Scratch::new(None).expect("a scratch directory");
            assert!(s.root.is_dir());
            s.root.clone()
        };
        assert!(!root.exists());
    }

    #[test]
    fn sizes_and_units_read_plainly() {
        assert_eq!(show(344_008.0, "bytes"), "344 KB");
        assert_eq!(show(2_900_000.0, "bytes"), "2.90 MB");
        assert_eq!(show(0.24, "ms"), "0.24 ms");
        assert_eq!(show(1500.0, "ms"), "1.50 s");
        assert_eq!(show(1.25, "x"), "1.25x");
    }
}
