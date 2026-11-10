// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `self update` (SPEC 12.4): fetch and admit the manifest's `ciginstaller` build, store its two
//! files, keep the running binary as `installer/previous/ciginstaller`, swap the new one in by
//! rename, re-link the shims to the new `cigi-shim`, start the new binary once as
//! `__selfcheck`, and commit. A failed self-check rolls the whole transaction back: the old
//! binary is the one in `bin/` again, and the error is `E1853`.
//!
//! `update` (12.2 step 1) calls [`offered`] to learn whether a newer installer exists and
//! [`run_locked`] to apply it; the caller re-executes `bin/ciginstaller` afterwards.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cig_compat::{Manifest, Target, Version};
use cigi_home::state::HistoryEntry;
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
    if cfg!(windows) {
        return Err(InstallerError::new(
            "E1872",
            "self update is not built for Windows yet",
        ));
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
                    .join("ciginstaller")
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
    if std::fs::metadata(&current).is_ok() {
        txn.mkdir_all(&previous_dir).map_err(ie)?;
        txn.link(&current, &previous_dir.join("ciginstaller"))
            .map_err(ie)?;
    }

    // the swap: each link is made beside the target and renamed over it, so `bin/ciginstaller`
    // is the old file or the new one and never anything between
    for n in ["ciginstaller", "cigi"] {
        txn.link(&store.object_path(&exe_obj), &shim_path(&home, n))
            .map_err(ie)?;
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
    let names = shim_names(&home, &state);
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    sync_shims(txn, &home, &refs, &shim_obj).map_err(ie)?;

    // The new binary must start before anything is committed. State is already written, so what
    // it checks is the home it would run in.
    state.save_in(txn).map_err(ie)?;
    txn.barrier().map_err(ie)?;
    selfcheck(&home, &current, want, have)?;

    save_and_flip(txn, &home, &state, yanked_in(m))
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
            Err(e) if e.raw_os_error() == Some(26) && tries < 100 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(5));
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
    if state.installer.healthy || state.installer.version != running.to_string() {
        return;
    }
    state.installer.healthy = true;
    state.installer.first_ok_run = Some(now_rfc3339());
    let Ok(mut txn) = Txn::begin(&cx.home, "installer healthy", lock) else {
        return;
    };
    if state.save_in(&mut txn).is_ok() {
        let _ = txn.commit();
    } else {
        let _ = txn.rollback();
    }
}
