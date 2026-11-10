// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `self update` (SPEC 12.4): fetch and admit the manifest's `ciginstaller` build, store its two
//! files, keep the running binary as `installer/previous/ciginstaller`, swap the new one in by
//! rename, re-link the shims to the new `cigi-shim`, start the new binary once as
//! `__selfcheck`, and commit. A failed self-check rolls the whole transaction back: the old
//! binary is the one in `bin/` again, and the error is `E1853`.
//!
//! **Windows (#8).** A running exe can be renamed but not replaced, so the swap differs in two
//! ways. (1) Replacing `bin\ciginstaller.exe` (and any shim a running tool holds) is attempted
//! through the journal first, which saves the old bytes before it tries; when Windows refuses
//! because the file is a running image, the file is renamed to `installer\previous\.aside-*`
//! and the new one placed. Whatever moment a kill lands in, the journal's undo puts the saved
//! old file back at `bin\ciginstaller.exe` (`Txn::move_path` is not used: its undo of a file does
//! not rename anything back). The kill between the rename and the placement has its own fault
//! point, `CIGI_FAULT=self-update-aside:1`. (2) `bin\cigi.exe` is the shim there, and a
//! committed update leaves `installer\pending-check`; while it is there, a new build that cannot
//! start is replaced by `installer\previous\ciginstaller.exe` by the shim (see
//! `cigi-shim/src/installer.rs`), and the next command this installer runs reconciles its state
//! ([`mark_healthy`]). The asides are swept at the start of a later update, with retries.
//!
//! `update` (12.2 step 1) calls [`offered`] to learn whether a newer installer exists and
//! [`run_locked`] to apply it; the caller re-executes `bin/ciginstaller` afterwards.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cig_compat::{Manifest, Target, Version};
use cigi_home::state::HistoryEntry;
use cigi_home::JournalError;
use cigi_home::{
    materialize, now_rfc3339, prune_txns, shim_path, sync_shims, Home, Journal, ObjectId,
    Provenance, ReceiptArtifact, ReceiptSource, ReceiptTrust, StagedFile, State, Txn,
};
use cigi_source::fetch::downloads_dir;
use cigi_source::{
    get_artifact, stage_artifact, sweep_downloads, Progress as SourceProgress, StagedArtifact,
};
use cigi_ui::{InstallerError, Report, EXIT_FAILED, EXIT_REFUSED};
use serde_json::json;

use super::{save_and_flip, settle, yanked_in};
use crate::cx::{Cx, ManifestMode};
use crate::errors::{bug, ie};

/// How long the new binary gets to answer `__selfcheck`.
const SELFCHECK_BUDGET: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Default)]
pub struct SelfArgs {
    /// Say what would happen; change nothing (`--check`, `--dry-run`).
    pub check: bool,
}

/// The version of the running installer.
pub fn running_version() -> Result<Version, InstallerError> {
    Version::parse(env!("CARGO_PKG_VERSION"))
        .map_err(|e| bug(format!("the crate version does not parse: {e}")))
}

/// The newest `ciginstaller` build the manifest offers this platform, with the carton that
/// carries it: not yanked (carton or build), and not a prerelease carton unless the channel
/// itself is a prerelease channel.
pub fn offered(m: &Manifest, target: &Target) -> Option<(Version, Version)> {
    let pre_ok = m.channel != "stable";
    m.cartons
        .values()
        .filter(|c| (pre_ok || !c.version.is_prerelease()) && !m.is_carton_yanked(&c.version))
        .filter_map(|c| c.products.get("ciginstaller").map(|v| (v.clone(), c)))
        .filter(|(v, _)| {
            !m.is_product_yanked("ciginstaller", v)
                && m.artifact("ciginstaller", v, target).is_some()
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(v, c)| (v, c.version.clone()))
}

/// The newer installer the manifest offers than the one running, if any. What `update` asks
/// before it does anything else.
pub fn newer_installer(m: &Manifest, target: &Target) -> Option<Version> {
    let have = running_version().ok()?;
    offered(m, target).map(|(v, _)| v).filter(|v| *v > have)
}

#[cfg(unix)]
fn same_file(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
        _ => false,
    }
}

#[cfg(not(unix))]
fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// Is the running binary the one in `bin/` (`ciginstaller`, or `cigi`, its other name)? A build
/// from a source tree is not, and never replaces itself.
pub fn is_managed(home: &Home) -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    ["ciginstaller", "cigi"]
        .iter()
        .any(|n| same_file(&exe, &shim_path(home, n)))
}

fn refuse_dev_build(home: &Home) -> Result<(), InstallerError> {
    if is_managed(home) {
        return Ok(());
    }
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    Err(InstallerError::new(
        "E1853",
        format!(
            "this is {exe}, not {}: a development build never replaces itself",
            shim_path(home, "ciginstaller").display()
        ),
    )
    .with_fix(format!(
        "{} self update",
        shim_path(home, "ciginstaller").display()
    ))
    .with_exit(EXIT_REFUSED))
}

struct Bar(Box<dyn cigi_ui::Progress>);

impl SourceProgress for Bar {
    fn set_total(&mut self, total: u64) {
        self.0.set_total(total);
    }
    fn advance(&mut self, bytes: u64) {
        self.0.advance(bytes);
    }
    fn done(&mut self) {
        self.0.abort();
    }
}

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn run(cx: &mut Cx, a: &SelfArgs) -> Result<Report, InstallerError> {
    if !a.check {
        cx.lock_and_recover()?;
    }
    run_locked(cx, a)
}

/// `self update` with the lock held and `recover` done (unless `check`).
pub fn run_locked(cx: &mut Cx, a: &SelfArgs) -> Result<Report, InstallerError> {
    let have = running_version()?;
    let mode = if a.check {
        ManifestMode::Daily
    } else {
        ManifestMode::Fresh
    };
    let m: Manifest = cx.manifest(mode)?.clone();
    let Some((want, carton)) = offered(&m, &cx.target) else {
        return Err(InstallerError::new(
            "E1815",
            format!(
                "the {} channel has no ciginstaller build for {}",
                m.channel, cx.target
            ),
        )
        .with_fix("ciginstaller carton list")
        .with_exit(EXIT_REFUSED));
    };
    if want <= have {
        return Ok(Report::done(format!("ciginstaller {have} is the newest"))
            .data(json!({ "from": have.to_string(), "to": have.to_string(), "changed": false })));
    }
    if a.check {
        return Ok(
            Report::done(format!("ciginstaller {want} is available (this is {have})"))
                .next("ciginstaller self update")
                .data(
                    json!({ "from": have.to_string(), "to": want.to_string(), "changed": false }),
                ),
        );
    }
    refuse_dev_build(&cx.home)?;
    update_to(cx, &m, &want, &carton, &have)
}

/// Pick the two files the installer's archive must hold, wherever the archive roots them.
fn pick_files(files: &[StagedFile]) -> Result<Vec<StagedFile>, InstallerError> {
    let suffix = std::env::consts::EXE_SUFFIX;
    let mut out = Vec::new();
    for name in ["ciginstaller", "cigi-shim"] {
        let want = format!("{name}{suffix}");
        let f = files
            .iter()
            .find(|f| f.path.file_name().is_some_and(|n| n == want.as_str()))
            .ok_or_else(|| {
                InstallerError::new(
                    "E1853",
                    format!("the ciginstaller release holds no {want}; nothing was changed"),
                )
                .with_exit(EXIT_FAILED)
            })?;
        out.push(StagedFile {
            path: PathBuf::from(format!("bin/{name}")),
            source: f.source.clone(),
            exec: true,
        });
    }
    Ok(out)
}

/// The file name the installer has in `bin/` and in `installer/previous/`.
fn exe_name() -> String {
    format!("ciginstaller{}", std::env::consts::EXE_SUFFIX)
}

/// `installer/pending-check`: a committed update the new build has not yet vouched for (Windows).
pub fn pending_check_path(home: &Home) -> PathBuf {
    cigi_shim::PENDING_CHECK
        .iter()
        .fold(home.root(), |p, s| p.join(s))
}

fn restored_path(home: &Home) -> PathBuf {
    cigi_shim::RESTORED
        .iter()
        .fold(home.root(), |p, s| p.join(s))
}

/// Windows said no because a process is running that file: access denied, a sharing violation
/// or a lock violation. Never on another platform.
fn blocked(e: &std::io::Error) -> bool {
    cfg!(windows) && matches!(e.raw_os_error(), Some(5 | 32 | 33))
}

/// Retry `f` while the answer is [`blocked`] (a scanner or a process still exiting holds the
/// file), backing off up to `patience`.
fn retry_blocked<T>(
    patience: Duration,
    mut f: impl FnMut() -> std::io::Result<T>,
) -> std::io::Result<T> {
    let end = Instant::now() + patience;
    let mut wait = Duration::from_millis(20);
    loop {
        match f() {
            Err(e) if blocked(&e) && Instant::now() < end => {
                std::thread::sleep(wait);
                wait = (wait * 2).min(Duration::from_millis(500));
            }
            r => return r,
        }
    }
}

fn is_aside(name: &str) -> bool {
    name.starts_with(".aside-") || name == "ciginstaller.failed.exe"
}

/// Remove what earlier updates left in `installer/previous/`: renamed-aside running images
/// (`.aside-*`) and a build the shim restored away from. The kept previous installer itself is
/// not touched. Returns how many could not be removed yet (a process still runs them).
/// Never fails an update by itself.
pub fn sweep_aside(home: &Home, patience: Duration) -> usize {
    let Ok(rd) = std::fs::read_dir(home.installer().join("previous")) else {
        return 0;
    };
    let mut left = 0;
    for e in rd.flatten() {
        if !is_aside(&e.file_name().to_string_lossy()) {
            continue;
        }
        let p = e.path();
        let gone = retry_blocked(patience, || {
            // a read-only attribute makes the delete fail with access denied
            if let Ok(md) = std::fs::metadata(&p) {
                let mut perm = md.permissions();
                if perm.readonly() {
                    #[allow(clippy::permissions_set_readonly_false)]
                    perm.set_readonly(false);
                    let _ = std::fs::set_permissions(&p, perm);
                }
            }
            std::fs::remove_file(&p)
        });
        if gone.is_err() {
            left += 1;
        }
    }
    left
}

/// Are the two files the same bytes? Lengths first; no inodes needed.
fn same_bytes(a: &Path, b: &Path) -> bool {
    let (Ok(x), Ok(y)) = (std::fs::metadata(a), std::fs::metadata(b)) else {
        return false;
    };
    x.len() == y.len() && matches!((std::fs::read(a), std::fs::read(b)), (Ok(p), Ok(q)) if p == q)
}

/// Put `from` at `to`, journaled. When Windows refuses the replace because `to` is a running
/// image, rename `to` into `aside_dir` and place `from` there instead. Returns whether it did.
///
/// The journal saves the old `to` before it tries the replace, so the refused attempt is still
/// on record: the undo of that record puts the old file back, whether a kill lands before the
/// rename, between the rename and the placement (`fault_after("self-update-aside")`), or after.
fn replace_or_aside(
    txn: &mut Txn,
    from: &Path,
    to: &Path,
    aside_dir: &Path,
) -> Result<bool, InstallerError> {
    match txn.link(from, to) {
        Ok(()) => Ok(false),
        Err(JournalError::Io { source, .. }) if blocked(&source) => {
            let name = to.file_name().map(|n| n.to_string_lossy().into_owned());
            let aside = aside_dir.join(format!(
                ".aside-{}-{}",
                txn.id(),
                name.as_deref().unwrap_or("file")
            ));
            let io = |p: &Path, e: std::io::Error| {
                InstallerError::new("E1835", format!("cannot move {}: {e}", p.display()))
            };
            retry_blocked(Duration::from_secs(3), || std::fs::rename(to, &aside))
                .map_err(|e| io(to, e))?;
            cigi_home::journal::fault_after("self-update-aside");
            retry_blocked(Duration::from_secs(3), || {
                std::fs::hard_link(from, to).or_else(|_| std::fs::copy(from, to).map(|_| ()))
            })
            .map_err(|e| io(to, e))?;
            Ok(true)
        }
        Err(e) => Err(ie(e)),
    }
}

fn update_to(
    cx: &mut Cx,
    m: &Manifest,
    want: &Version,
    carton: &Version,
    have: &Version,
) -> Result<Report, InstallerError> {
    // fetch and admit, outside the transaction: a failure here leaves the home untouched
    let _ = sweep_downloads(&cx.home);
    let size = m.artifact("ciginstaller", want, &cx.target).map(|x| x.size);
    let mut bar = Bar(cx
        .ui
        .progress(&format!("fetching ciginstaller {want}"), size));
    let staged: StagedArtifact = get_artifact(
        cx.source.as_ref(),
        &cx.trust,
        m,
        "ciginstaller",
        want,
        &cx.home,
        &mut bar,
    )
    .map_err(ie)?;
    let dir = downloads_dir(&cx.home).join(format!(".stage-{}-ciginstaller", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| {
        InstallerError::new("E1835", format!("cannot create {}: {e}", dir.display()))
    })?;
    let _scratch = Scratch(dir.clone());
    let unpacked = stage_artifact(m, &staged, &dir).map_err(ie)?;
    let files = pick_files(&unpacked)?;

    // a running image an earlier update renamed aside may still be held for a moment
    if sweep_aside(&cx.home, Duration::from_secs(3)) > 0 {
        return Err(InstallerError::new(
            "E1853",
            format!(
                "an earlier ciginstaller is still running from {}; nothing was changed",
                cx.home.installer().join("previous").display()
            ),
        )
        .with_fix("close running ciginstaller windows, then run `ciginstaller self update` again")
        .with_exit(EXIT_FAILED));
    }
    let mut txn = cx.begin(&format!("self update {have} to {want}"))?;
    let applied = apply(cx, &mut txn, m, want, carton, have, &staged, &files);
    let txn_id = txn.id().to_string();
    settle(txn, applied)?;
    let _ = prune_txns(&cx.home, cx.lock_ref()?, Duration::from_secs(7 * 24 * 3600));
    let _ = std::fs::remove_file(&staged.path);
    cx.state = State::load(&cx.home).map_err(ie)?;
    Ok(
        Report::done(format!("ciginstaller updated {have} -> {want}"))
            .detail(format!(
                "trust: {}{}",
                staged.verified.level.as_str(),
                if staged.verified.independent {
                    " (two sources agree)"
                } else {
                    " (a mirror: the sources are not independent)"
                }
            ))
            .detail(format!(
                "{have} is kept at {}",
                cx.home
                    .installer()
                    .join("previous")
                    .join(exe_name())
                    .display()
            ))
            .next("ciginstaller --version")
            .data(json!({
                "from": have.to_string(), "to": want.to_string(), "changed": true, "txn": txn_id,
                "trust": staged.verified.level.as_str(),
            })),
    )
}

use super::switch::shim_names;

#[allow(clippy::too_many_arguments)]
fn apply(
    cx: &mut Cx,
    txn: &mut Txn,
    m: &Manifest,
    want: &Version,
    carton: &Version,
    have: &Version,
    staged: &StagedArtifact,
    files: &[StagedFile],
) -> Result<(), InstallerError> {
    let home = cx.home.clone();
    let art = &staged.art;
    let prov = Provenance {
        from_carton: Some(carton.to_string()),
        source: ReceiptSource {
            repo: art.repo.clone(),
            tag: art.tag.clone(),
            asset: art.asset.clone(),
            origin: staged.origin.clone(),
        },
        artifact: ReceiptArtifact {
            sha256: staged.verified.sha256.clone(),
            size: art.size,
        },
        trust: ReceiptTrust {
            level: staged.verified.level.as_str().to_string(),
            independent: staged.verified.independent,
            attested_by: staged.verified.attested_by.clone(),
            manifest_sequence: m.sequence,
        },
        shims: Vec::new(),
        license: m
            .product("ciginstaller", want)
            .and_then(|p| p.license.clone()),
    };
    txn.mkdir_all(&home.toolchains()).map_err(ie)?;
    let receipt = materialize(txn, &home, "ciginstaller", want, files, prov).map_err(ie)?;
    let object = |path: &str| -> Result<ObjectId, InstallerError> {
        receipt
            .files
            .iter()
            .find(|f| f.path == path)
            .map(|f| ObjectId::new(f.sha256.clone(), f.exec))
            .ok_or_else(|| bug(format!("the ciginstaller receipt lost {path}")))
    };
    let (exe_obj, shim_obj) = (object("bin/ciginstaller")?, object("bin/cigi-shim")?);
    let store = home.object_store();
    store.verify(&exe_obj).map_err(ie)?;

    // the running binary is kept (journaled: a rollback removes the link again)
    let current = shim_path(&home, "ciginstaller");
    let previous_dir = home.installer().join("previous");
    if cfg!(windows) || std::fs::metadata(&current).is_ok() {
        txn.mkdir_all(&previous_dir).map_err(ie)?;
    }
    if std::fs::metadata(&current).is_ok() {
        txn.link(&current, &previous_dir.join(exe_name()))
            .map_err(ie)?;
    }
    if cfg!(windows) {
        // the shim reads this after the commit: until the new build has run once, a build that
        // cannot start is replaced by the kept one (journaled, so a rollback removes it)
        txn.write(
            &pending_check_path(&home),
            format!("{want}\n").as_bytes(),
            None,
        )
        .map_err(ie)?;
    }

    // the swap: each link is made beside the target and renamed over it, so `bin/ciginstaller`
    // is the old file or the new one and never anything between. On Windows a running image
    // cannot be renamed over: it is renamed aside first (see `replace_or_aside`).
    let new_exe = store.object_path(&exe_obj);
    if cfg!(windows) {
        replace_or_aside(txn, &new_exe, &current, &previous_dir)?;
    } else {
        for n in super::switch::installer_names() {
            txn.link(&new_exe, &shim_path(&home, n)).map_err(ie)?;
        }
    }

    // state, then the shims follow the new `cigi-shim` object
    let mut state = cx.state.clone();
    let now = now_rfc3339();
    state.installer.version = want.to_string();
    state.installer.previous = Some(have.to_string());
    state.installer.healthy = false;
    state.installer.first_ok_run = None;
    state.push_history(HistoryEntry {
        id: txn.id().to_string(),
        at: now,
        what: "self update".into(),
        from: Some(have.to_string()),
        to: Some(want.to_string()),
        txn: txn.id().to_string(),
        ok: true,
    });
    // on Windows `shim_names` includes `cigi`: the shim is what restores a build that cannot start
    let names = shim_names(&home, &state);
    if cfg!(windows) {
        let obj = store.object_path(&shim_obj);
        store.verify(&shim_obj).map_err(ie)?;
        for n in &names {
            if !cigi_home::shims::shim_name_ok(n) {
                return Err(bug(format!("`{n}` is not a shim name")));
            }
            let to = shim_path(&home, n);
            // an unchanged shim stays: a tool running through it keeps it open
            if !same_bytes(&obj, &to) {
                replace_or_aside(txn, &obj, &to, &previous_dir)?;
            }
        }
    } else {
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        sync_shims(txn, &home, &refs, &shim_obj).map_err(ie)?;
    }

    // The new binary must start before anything is committed. State is already written, so what
    // it checks is the home it would run in.
    state.save_in(txn).map_err(ie)?;
    txn.barrier().map_err(ie)?;
    selfcheck(&home, &current, want, have)?;

    save_and_flip(txn, &home, &state, yanked_in(m))
}

/// The file was written a moment ago and something still holds it: ETXTBSY on Unix; on Windows
/// a sharing violation or access denied while a scanner looks at the new file.
fn spawn_busy(e: &std::io::Error) -> bool {
    if cfg!(windows) {
        blocked(e)
    } else {
        e.raw_os_error() == Some(26)
    }
}

/// Start `exe __selfcheck`: it must exit 0 within the budget and name the version it was meant
/// to be. Anything else is `E1853`, and the caller rolls back.
fn selfcheck(
    home: &Home,
    exe: &Path,
    want: &Version,
    have: &Version,
) -> Result<(), InstallerError> {
    let failed = |why: String| {
        InstallerError::new(
            "E1853",
            format!("ciginstaller {want} failed its self-check ({why}); ciginstaller {have} was put back"),
        )
        .with_exit(EXIT_FAILED)
    };
    let mut tries = 0;
    let mut child = loop {
        let mut cmd = Command::new(exe);
        cmd.arg("__selfcheck")
            .env("CIGSCRIPT_HOME", home.root())
            .env_remove("CIGI_FAULT")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        match cmd.spawn() {
            Ok(c) => break c,
            // a file written a moment ago can still be open in a child another thread forked
            Err(e) if spawn_busy(&e) && tries < 100 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(if cfg!(windows) { 30 } else { 5 }));
            }
            Err(e) => return Err(failed(format!("it did not start: {e}"))),
        }
    };
    let mut out = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(o) = out.as_mut() {
            let _ = o.take(64 * 1024).read_to_string(&mut s);
        }
        s
    });
    let deadline = Instant::now() + SELFCHECK_BUDGET;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(failed("it did not answer in time".into()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => return Err(failed(format!("it could not be waited for: {e}"))),
        }
    };
    let said = reader.join().unwrap_or_default();
    if !status.success() {
        return Err(failed(format!(
            "exit {}",
            status
                .code()
                .map_or("by signal".to_string(), |c| c.to_string())
        )));
    }
    if !said.contains(&want.to_string()) {
        return Err(failed(format!(
            "it says {:?}, not {want}",
            said.lines().next().unwrap_or("")
        )));
    }
    Ok(())
}

/// The hidden `__selfcheck`: print the version, check the home loads and is safe to use.
pub fn selfcheck_report(cx: &Cx) -> Result<String, InstallerError> {
    cx.home.check_permissions().map_err(ie)?;
    Ok(format!("ciginstaller {}", env!("CARGO_PKG_VERSION")))
}

/// After the first successful command of a freshly installed build, `state.installer.healthy`
/// turns true (SPEC 12.4: the previous binary is then no longer needed for a restore). Best
/// effort: never fails a command, never waits for the lock, only runs as the managed binary.
pub fn mark_healthy(cx: &Cx) {
    let Ok(running) = running_version() else {
        return;
    };
    if !is_managed(&cx.home) {
        return;
    }
    // the update that ran this build may still be exiting: one try, no waiting
    sweep_aside(&cx.home, Duration::ZERO);
    let taken;
    let lock = match cx.lock_ref() {
        Ok(l) => l,
        Err(_) => match cx.home.lock_with(Duration::ZERO, &mut |_| {}) {
            Ok(l) => {
                taken = l;
                &taken
            }
            Err(_) => return,
        },
    };
    if cigi_home::list_txns(&cx.home).iter().any(|t| {
        matches!(
            t.status,
            cigi_home::TxnStatus::Open | cigi_home::TxnStatus::Unreadable
        )
    }) {
        return;
    }
    let Ok(mut state) = State::load(&cx.home) else {
        return;
    };
    let restored = std::fs::read_to_string(restored_path(&cx.home))
        .ok()
        .map(|t| t.trim().to_string());
    let reconciled = restored
        .as_deref()
        .is_some_and(|failed| reconcile_restored(&mut state, &running, failed));
    if !reconciled && (state.installer.healthy || state.installer.version != running.to_string()) {
        return;
    }
    state.installer.healthy = true;
    state.installer.first_ok_run = Some(now_rfc3339());
    let Ok(mut txn) = Txn::begin(&cx.home, "installer healthy", lock) else {
        return;
    };
    for marker in [pending_check_path(&cx.home), restored_path(&cx.home)] {
        if marker.exists() {
            let _ = txn.delete(&marker);
        }
    }
    if state.save_in(&mut txn).is_ok() {
        let _ = txn.commit();
    } else {
        let _ = txn.rollback();
    }
}

/// The shim put the previous installer back after `failed` could not start (Windows): bring
/// `state` in line with the build that is running. True when it did.
fn reconcile_restored(state: &mut State, running: &Version, failed: &str) -> bool {
    let run = running.to_string();
    if state.installer.version == run || state.installer.previous.as_deref() != Some(run.as_str()) {
        return false;
    }
    state.installer.version = run.clone();
    state.installer.previous = None;
    state.push_history(HistoryEntry {
        id: format!("restored-{failed}"),
        at: now_rfc3339(),
        what: "self update restored".into(),
        from: Some(failed.to_string()),
        to: Some(run),
        txn: String::new(),
        ok: false,
    });
    true
}

#[cfg(test)]
#[path = "self_update_tests.rs"]
mod tests;
