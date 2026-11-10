// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `Cx`: everything a flow needs, built once per command (SPEC 2.6). Flows return a `Report`;
//! the only things they print are the progress bar and the notes `Cx` itself emits (a recovered
//! transaction, a lock wait).

use std::cell::OnceCell;
use std::ffi::OsString;
use std::path::PathBuf;

use cig_compat::{Host, Manifest, Target};
use cigi_home::{recover, Config, Home, InstallerLock, RecoverOutcome, State, Txn};
use cigi_source::fetch::cache_age_secs;
use cigi_source::{
    get_manifest_with, source_from_config, Admitted, DigestTrust, GhAttest, ManifestOpts, Source,
    TrustLevel,
};
use cigi_ui::{Icon, InstallerError, Report, Ui, EXIT_FAILED};

use crate::errors::{bug, ie};
use crate::project::{self, ProjectCtx};

/// The flags every command shares.
#[derive(Debug, Clone, Default)]
pub struct Globals {
    pub yes: bool,
    pub offline: bool,
    pub home: Option<PathBuf>,
    /// `--self-updated <version>`: this run is the new build a self-update just started.
    pub self_updated: Option<String>,
}

/// How fresh a manifest has to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestMode {
    /// Always fetch (install).
    Fresh,
    /// The cache when it is under 24 hours old, else fetch, else the cache with a note (`carton list`).
    Daily,
    /// Never touch the network (`which`, `check` of a pin).
    CacheOnly,
}

pub struct Cx {
    pub ui: Ui,
    pub home: Home,
    pub cfg: Config,
    pub state: State,
    pub source: Box<dyn Source>,
    pub trust: DigestTrust,
    pub host: Host,
    pub target: Target,
    /// What the walk up from `cwd` found. Held, not raised: a bad pin file only stops the
    /// commands that need the project (`which`, `__resolve`), see [`Cx::project`].
    project: Result<Option<ProjectCtx>, InstallerError>,
    pub manifest: OnceCell<Admitted<Manifest>>,
    pub globals: Globals,
    pub cwd: PathBuf,
    lock: Option<InstallerLock>,
}

/// `allow_single_source = true`: a mirror that serves the manifest, the checksum and the file
/// counts as checksummed (off by default: one operator is not two sources).
fn single_source(cfg: &Config) -> bool {
    cfg.get("allow_single_source") == "true"
}

fn floor_of(cfg: &Config) -> TrustLevel {
    TrustLevel::parse(&cfg.get("trust")).unwrap_or(TrustLevel::Checksummed)
}

impl Cx {
    pub fn new(ui: Ui, globals: Globals, cwd: Option<PathBuf>) -> Result<Cx, InstallerError> {
        let override_home: Option<OsString> =
            globals.home.as_ref().map(|h| h.clone().into_os_string());
        let home = Home::discover_with(&|k| {
            if k == "CIGSCRIPT_HOME" {
                if let Some(h) = &override_home {
                    return Some(h.clone());
                }
            }
            std::env::var_os(k)
        })
        .map_err(ie)?;
        let cfg = Config::load(&home).map_err(ie)?;
        let state = State::load(&home).map_err(ie)?;
        let source = source_from_config(&cfg);
        let trust = DigestTrust::new(floor_of(&cfg)).with_single_source(single_source(&cfg));
        let target = Target::host();
        let cwd = match cwd {
            Some(c) => c,
            None => {
                std::env::current_dir().map_err(|e| bug(format!("no working directory: {e}")))?
            }
        };
        let project = project::discover(&cwd, &home.root(), cigi_shim::user_home().as_deref());
        Ok(Cx {
            ui,
            home,
            cfg,
            state,
            source,
            trust,
            host: Host::from_target(&target),
            target,
            project,
            manifest: OnceCell::new(),
            globals,
            cwd,
            lock: None,
        })
    }

    /// The project the working directory belongs to, or the reason its pin file cannot be used.
    pub fn project(&self) -> Result<Option<ProjectCtx>, InstallerError> {
        self.project.clone()
    }

    /// Raise the trust floor for this command (`--require`); never lowers it.
    pub fn require_trust(&mut self, level: TrustLevel) {
        let floor = floor_of(&self.cfg).max(level);
        self.trust = DigestTrust::new(floor).with_single_source(single_source(&self.cfg));
    }

    /// The channel in force: the setting, which `--channel` replaces in `cfg` before this is asked.
    pub fn channel(&self) -> String {
        self.cfg.get("channel")
    }

    /// Say a one-line note on stderr/stdout in the grammar.
    pub fn note(&self, line: &str) {
        self.ui.say(&Report::new(Icon::Note, line));
    }

    /// Layout and permission checks, the lock, `recover`, then a fresh read of `state.json`
    /// (every mutating command starts here, SPEC 12).
    ///
    /// Idempotent: when this `Cx` already holds the lock (a flow that calls another flow, the
    /// slow path's second install) it only reloads `state.json`; taking the lock a second time
    /// from the same process would wait on itself.
    pub fn lock_and_recover(&mut self) -> Result<(), InstallerError> {
        if self.lock.is_some() {
            self.state = State::load(&self.home).map_err(ie)?;
            return Ok(());
        }
        // permissions are checked before anything is written, and again once the layout exists
        self.home.check_permissions().map_err(ie)?;
        self.home.ensure_layout().map_err(ie)?;
        self.home.check_permissions().map_err(ie)?;
        let ui = &self.ui;
        let lock = self
            .home
            .lock_with(cigi_home::lock::LOCK_WAIT, &mut |pid| {
                let who = pid.map(|p| format!(" (pid {p})")).unwrap_or_default();
                ui.say(&Report::new(
                    Icon::Note,
                    format!("waiting for another ciginstaller{who}"),
                ));
            })
            .map_err(ie)?;
        let recovered = recover(&self.home, &lock).map_err(ie)?;
        let mut problem: Option<InstallerError> = None;
        for r in &recovered {
            let what = match &r.outcome {
                RecoverOutcome::RolledBack { .. } => {
                    format!("an interrupted `{}` was undone ({})", r.label, r.txn)
                }
                RecoverOutcome::RolledForward => {
                    format!("an interrupted `{}` was finished ({})", r.label, r.txn)
                }
                RecoverOutcome::Corrupt { why } => {
                    format!(
                        "the journal of `{}` ({}) cannot be recovered: {why}",
                        r.label, r.txn
                    )
                }
                RecoverOutcome::Incomplete { failed } => format!(
                    "the undo of `{}` ({}) is incomplete: {} step(s) failed",
                    r.label,
                    r.txn,
                    failed.len()
                ),
                RecoverOutcome::Settled { why } => format!(
                    "the journal of the finished `{}` ({}) was tidied: {why}",
                    r.label, r.txn
                ),
            };
            if r.is_problem() && problem.is_none() {
                problem = Some(
                    InstallerError::new(r.code(), what.clone())
                        .with_fix("ciginstaller doctor")
                        .with_exit(EXIT_FAILED),
                );
            }
            self.ui.say(&Report::new(Icon::Note, what).code(r.code()));
            for n in &r.notes {
                self.ui.say(&Report::new(Icon::Note, n.clone()));
            }
        }
        if let Some(p) = problem {
            return Err(p);
        }
        self.lock = Some(lock);
        self.state = State::load(&self.home).map_err(ie)?;
        Ok(())
    }

    /// Let go of the installer lock (a no-op when it is not held). The slow path calls this
    /// before it spawns the tool, so a long run of the tool does not block other installers.
    pub fn release_lock(&mut self) {
        self.lock = None;
    }

    /// Open the transaction of a flow. `lock_and_recover` must have run.
    pub fn begin(&self, label: &str) -> Result<Txn, InstallerError> {
        let lock = self
            .lock
            .as_ref()
            .ok_or_else(|| bug("a transaction was opened without the installer lock"))?;
        Txn::begin(&self.home, label, lock).map_err(ie)
    }

    pub fn lock_ref(&self) -> Result<&InstallerLock, InstallerError> {
        self.lock
            .as_ref()
            .ok_or_else(|| bug("the installer lock is not held"))
    }

    /// The admitted manifest of the channel, fetched at most once per command.
    /// The manifest of `channel`, fetched and admitted now, with the fetch error as it came
    /// (`channel <name>` tells a channel with no manifest from a failed download).
    pub fn fetch_manifest(&self, channel: &str) -> Result<Admitted<Manifest>, FetchRefused> {
        let seen = self.seen_sequence(channel);
        let opts = ManifestOpts {
            offline: self.globals.offline,
            seen_sequence: seen,
            now: now_override(),
            fetch_attestation: GhAttest.available_cached(),
        };
        let kept = CacheCopy::take(&self.home, channel);
        let admitted = get_manifest_with(
            self.source.as_ref(),
            &self.trust,
            &self.home,
            channel,
            &opts,
        )
        .map_err(FetchRefused::Fetch)?;
        // A different document at a sequence already seen is refused, and the cache goes back to
        // the document it held (the fetch has already written the new one).
        if let Err(e) = self.check_digest_record(channel, &admitted) {
            kept.restore();
            return Err(FetchRefused::Replay(e));
        }
        Ok(admitted)
    }

    pub fn manifest(&self, mode: ManifestMode) -> Result<&Manifest, InstallerError> {
        if let Some(m) = self.manifest.get() {
            return Ok(m);
        }
        let channel = self.channel();
        let kept = CacheCopy::take(&self.home, &channel);
        let seen = self.seen_sequence(&channel);
        let opts = |offline: bool| ManifestOpts {
            offline,
            seen_sequence: seen,
            now: now_override(),
            fetch_attestation: GhAttest.available_cached(),
        };
        let get = |offline: bool| {
            get_manifest_with(
                self.source.as_ref(),
                &self.trust,
                &self.home,
                &channel,
                &opts(offline),
            )
        };
        let offline = self.globals.offline;
        let admitted = match mode {
            _ if offline => get(true).map_err(ie)?,
            ManifestMode::CacheOnly => get(true).map_err(ie)?,
            ManifestMode::Fresh => get(false).map_err(ie)?,
            ManifestMode::Daily => {
                let fresh = cache_age_secs(&self.home, &channel, now_override())
                    .is_some_and(|a| (0..24 * 3600).contains(&a));
                if fresh {
                    get(true).map_err(ie)?
                } else {
                    match get(false) {
                        Ok(m) => m,
                        Err(e) => match get(true) {
                            Ok(m) => {
                                self.note(&format!("using the cached manifest: {e}"));
                                m
                            }
                            Err(_) => return Err(ie(e)),
                        },
                    }
                }
            }
        };
        if let Err(e) = self.check_digest_record(&channel, &admitted) {
            kept.restore();
            return Err(e);
        }
        for n in admitted.notes() {
            self.ui.say(&Report::new(Icon::Note, n.clone()));
        }
        let _ = self.manifest.set(admitted);
        self.manifest
            .get()
            .map(|a| a.get())
            .ok_or_else(|| bug("the manifest cell is empty after it was set"))
    }

    /// The cached manifest, read without the network and without being remembered: a later
    /// `manifest(Fresh)` in the same command still fetches. `None` when there is no usable cache.
    pub fn peek_manifest(&self) -> Option<Manifest> {
        if let Some(m) = self.manifest.get() {
            return Some(m.get().clone());
        }
        let channel = self.channel();
        let seen = self.seen_sequence(&channel);
        let opts = ManifestOpts {
            offline: true,
            seen_sequence: seen,
            now: now_override(),
            fetch_attestation: false,
        };
        let admitted = get_manifest_with(
            self.source.as_ref(),
            &self.trust,
            &self.home,
            &channel,
            &opts,
        )
        .ok()?;
        // the resolver, the switch checks and `check` read this: a cached document that is not
        // the one recorded for its sequence is not read at all
        self.check_digest_record(&channel, &admitted).ok()?;
        Some(admitted.get().clone())
    }

    /// The lowest sequence a fetched manifest of `channel` may have (SPEC 7.2): the higher of what
    /// `state.json` recorded (one channel's, the last installed from) and the sequence of the
    /// channel's own cache, so switching channels never resets the floor to "first use".
    fn seen_sequence(&self, channel: &str) -> Option<u64> {
        let state = self
            .state
            .manifest
            .as_ref()
            .filter(|m| m.channel == channel)
            .map(|m| m.sequence);
        let cached = cigi_source::fetch::read_fetched(&self.home, channel).map(|m| m.sequence);
        state.max(cached)
    }

    /// Record the digest of the manifest just admitted for `channel`, so a different document at
    /// the same sequence is refused later (E1824). The caller saves `state` in its transaction.
    pub fn record_manifest_digest(&self, state: &mut State, channel: &str, sequence: u64) {
        if let Some(meta) = cigi_source::fetch::read_fetched(&self.home, channel) {
            let entry = state
                .extra
                .entry(DIGESTS_KEY.to_string())
                .or_insert_with(|| serde_json::json!({}));
            if let Some(o) = entry.as_object_mut() {
                o.insert(
                    digest_key(channel, sequence),
                    serde_json::json!(meta.sha256),
                );
            }
        }
    }

    /// A different document at a sequence already recorded for the channel is refused (E1824).
    pub fn check_digest_record(&self, channel: &str, m: &Manifest) -> Result<(), InstallerError> {
        let Some(meta) = cigi_source::fetch::read_fetched(&self.home, channel) else {
            return Ok(());
        };
        let key = digest_key(channel, m.sequence);
        let known = self
            .state
            .extra
            .get(DIGESTS_KEY)
            .and_then(|v| v.get(&key))
            .and_then(|v| v.as_str());
        match known {
            Some(d) if d != meta.sha256 => Err(InstallerError::new(
                "E1824",
                format!(
                    "the {channel} manifest at sequence {} is a different document from the one seen before",
                    m.sequence
                ),
            )
            .with_hint("a manifest sequence names exactly one document")
            .with_fix("ciginstaller carton list --offline")),
            _ => Ok(()),
        }
    }

    /// The pin_fetch policy for the slow path: may a missing carton be fetched now?
    pub fn may_fetch_pinned(&self, what: &str) -> Result<bool, InstallerError> {
        if self.globals.offline {
            return Ok(false);
        }
        match self.cfg.get("pin_fetch").as_str() {
            "auto" => Ok(true),
            "never" => Ok(false),
            _ => {
                if !(self.ui.tty_in && self.ui.tty_out) && !self.globals.yes {
                    return Ok(false);
                }
                let q = cigi_ui::Question::yes_no(&format!("Fetch {what} now?"), true);
                Ok(self.ui.ask(&q).is_yes(&q))
            }
        }
    }
}

/// `CIGI_NOW` (epoch seconds) is honored only in builds with fault injection, so a release build
/// cannot be told what time it is.
#[cfg(feature = "fault-injection")]
pub(crate) fn now_override() -> Option<i64> {
    std::env::var("CIGI_NOW").ok()?.parse().ok()
}
#[cfg(not(feature = "fault-injection"))]
pub(crate) fn now_override() -> Option<i64> {
    None
}

/// Where `state.json` keeps the sha256 of each admitted manifest, keyed `channel:sequence`.
pub const DIGESTS_KEY: &str = "manifest_digests";

pub fn digest_key(channel: &str, sequence: u64) -> String {
    format!("{channel}:{sequence}")
}

/// Why `fetch_manifest` gave nothing.
#[derive(Debug)]
pub enum FetchRefused {
    /// The fetch itself failed, as the source reported it.
    Fetch(cigi_source::FetchError),
    /// The document came, and is not the one recorded for its sequence (E1824).
    Replay(InstallerError),
}

impl FetchRefused {
    pub fn into_error(self) -> InstallerError {
        match self {
            FetchRefused::Fetch(e) => ie(e),
            FetchRefused::Replay(e) => e,
        }
    }
}

/// The three files of a channel's manifest cache as they were, to put back when a document
/// that was fetched and written turns out to be a replay.
struct CacheCopy {
    dir: PathBuf,
    files: Vec<(&'static str, Option<Vec<u8>>)>,
}

impl CacheCopy {
    const NAMES: [&'static str; 3] = ["manifest.toml", "manifest.toml.sha256", "fetched.json"];

    fn take(home: &Home, channel: &str) -> CacheCopy {
        let dir = cigi_source::fetch::manifest_cache_dir(home, channel);
        let files = Self::NAMES
            .iter()
            .map(|n| (*n, std::fs::read(dir.join(n)).ok()))
            .collect();
        CacheCopy { dir, files }
    }

    fn restore(&self) {
        for (name, bytes) in &self.files {
            let p = self.dir.join(name);
            match bytes {
                Some(b) => {
                    let tmp = self.dir.join(format!(".{name}.restore"));
                    if std::fs::write(&tmp, b).is_ok() && std::fs::rename(&tmp, &p).is_err() {
                        let _ = std::fs::remove_file(&tmp);
                    }
                }
                None => {
                    let _ = std::fs::remove_file(&p);
                }
            }
        }
    }
}
