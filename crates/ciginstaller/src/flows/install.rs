// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `install` (SPEC 12.1): resolve the target from one carton, fetch and admit what is missing,
//! then one transaction: toolchains, humidor mirror, shims, PATH block, state, the index flip.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use cig_compat::{
    check, fix_command, Carton, Combination, Manifest, Origin, Pin, Severity, Version,
};
use cigi_home::state::{CartonRecord, HistoryEntry, ManifestInfo, Stick};
use cigi_home::{
    add_path_block, blocks_record, materialize, mirror_humidor, now_rfc3339, path_block, platform,
    prune_txns, read_receipt, recorded_registry, registry_line, registry_path_add,
    registry_path_has, registry_record, registry_supported, shim_path, sync_shims, ObjectId,
    PathChange, Provenance, ReceiptArtifact, ReceiptSource, ReceiptTrust, RegistryChange, Shell,
    ShimRef, StagedFile, State, Txn,
};
use cigi_source::fetch::downloads_dir;
use cigi_source::{
    get_artifact, stage_artifact, sweep_downloads, Progress as SourceProgress, StagedArtifact,
    TrustLevel,
};
use cigi_ui::{Icon, InstallerError, Question, Report, EXIT_REFUSED, EXIT_USAGE};
use serde_json::json;

use super::{present, save_and_flip, settle};
use crate::cx::{Cx, ManifestMode};
use crate::errors::{bug, ie};

#[derive(Debug, Clone)]
pub enum What {
    Carton,
    Pack(String),
    Pick(Vec<String>),
    Stick(String),
}

#[derive(Debug, Clone)]
pub struct InstallArgs {
    pub what: What,
    /// `--carton`: an exact carton or a pin (`0.1`).
    pub carton: Option<String>,
    pub no_path: bool,
    pub shell: Option<String>,
    pub allow_yanked: bool,
    pub dry_run: bool,
    /// `--preset` or the answer to the preset question: recorded in config with the install.
    pub preset: Option<String>,
    /// The answer to the crash-report consent question (`reports`), recorded with the install.
    pub reports: Option<String>,
    /// Why this install happens (`install carton`, `pin fetch`), recorded in `state.json`.
    pub reason: Option<String>,
    /// The first-run questions this run settled (asked, handed in, or already set), recorded
    /// with the install so they are not asked again (#22).
    pub first_run: Option<FirstRun>,
}

/// `state.json` key: which first-run questions have been answered, and how.
pub const FIRST_RUN_KEY: &str = "first_run";

/// The questions that are asked once on a machine, in the order they are asked.
pub const ONCE: [&str; 2] = ["preset", "reports"];

/// What one run settled of the once-only questions: id to value, and id to how
/// (`asked`, `answers` or `already-set`).
#[derive(Debug, Clone, Default)]
pub struct FirstRun {
    pub answers: BTreeMap<String, String>,
    pub how: BTreeMap<String, String>,
}

impl FirstRun {
    pub fn is_empty(&self) -> bool {
        self.answers.is_empty()
    }
}

/// The recorded answers of this machine (empty when none) and whether every question has one.
pub fn first_run_of(state: &State) -> (BTreeMap<String, String>, bool) {
    let answers: BTreeMap<String, String> = state
        .extra
        .get(FIRST_RUN_KEY)
        .and_then(|v| v.get("answers"))
        .and_then(|v| v.as_object())
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default();
    let done = ONCE.iter().all(|id| answers.contains_key(*id));
    (answers, done)
}

/// Has a first-run record been started on this machine?
pub fn first_run_started(state: &State) -> bool {
    state.extra.contains_key(FIRST_RUN_KEY)
}

/// The `first_run` object `status --json` and the install's data carry.
pub fn first_run_view(state: &State) -> serde_json::Value {
    let (answers, done) = first_run_of(state);
    let pending: Vec<&str> = ONCE
        .iter()
        .copied()
        .filter(|id| !answers.contains_key(*id))
        .collect();
    json!({ "done": done, "pending": pending, "answers": answers })
}

/// Fold this run's settled questions into the machine's record (inside the caller's transaction).
fn record_first_run(state: &mut State, fr: &FirstRun) {
    if fr.is_empty() {
        return;
    }
    let (mut answers, _) = first_run_of(state);
    let mut how: BTreeMap<String, String> = state
        .extra
        .get(FIRST_RUN_KEY)
        .and_then(|v| v.get("how"))
        .and_then(|v| v.as_object())
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default();
    for (k, v) in &fr.answers {
        answers.insert(k.clone(), v.clone());
    }
    for (k, v) in &fr.how {
        how.insert(k.clone(), v.clone());
    }
    let done = ONCE.iter().all(|id| answers.contains_key(*id));
    state.extra.insert(
        FIRST_RUN_KEY.to_string(),
        json!({ "answers": answers, "how": how, "done": done, "at": now_rfc3339() }),
    );
}

/// The answers of an install on a machine that already has everything: no download, so only the
/// settings and the record are written, in a transaction of their own.
fn record_answers(cx: &mut Cx, a: &InstallArgs) -> Result<(), InstallerError> {
    if a.dry_run || (a.preset.is_none() && a.reports.is_none() && a.first_run.is_none()) {
        return Ok(());
    }
    cx.cfg.reread(&cx.home).map_err(ie)?;
    let mut state = State::load(&cx.home).map_err(ie)?;
    if let Some(fr) = &a.first_run {
        record_first_run(&mut state, fr);
    }
    let mut txn = cx.begin("first-run answers")?;
    let mut write = || -> Result<(), InstallerError> {
        for (key, value) in [("preset", &a.preset), ("reports", &a.reports)] {
            if let Some(v) = value {
                cx.cfg.set_in(&mut txn, key, v).map_err(ie)?;
            }
        }
        state.save_in(&mut txn).map_err(ie)
    };
    let r = write();
    settle(txn, r)?;
    cx.state = state;
    Ok(())
}

impl InstallArgs {
    pub fn carton(version: Option<String>) -> InstallArgs {
        InstallArgs {
            what: What::Carton,
            carton: version,
            no_path: false,
            shell: None,
            allow_yanked: false,
            dry_run: false,
            preset: None,
            reports: None,
            reason: None,
            first_run: None,
        }
    }
}

/// Adapts the UI's bar to the source crate's progress trait.
pub(super) struct Bar(pub Box<dyn cigi_ui::Progress>);

impl SourceProgress for Bar {
    fn set_total(&mut self, total: u64) {
        self.0.set_total(total);
    }
    fn advance(&mut self, bytes: u64) {
        self.0.advance(bytes);
    }
    fn done(&mut self) {
        // done is called on failure too; the flow prints its own result line
        self.0.abort();
    }
}

/// A scratch directory removed on drop.
pub(super) struct Scratch(pub PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub(super) struct Fetched {
    pub tool: String,
    pub version: Version,
    pub staged: StagedArtifact,
    pub files: Vec<StagedFile>,
    _scratch: Scratch,
}

fn usage(code: &'static str, msg: impl Into<String>, fix: &str) -> InstallerError {
    InstallerError::new(code, msg)
        .with_fix(fix)
        .with_exit(EXIT_USAGE)
}

fn refused(code: &'static str, msg: impl Into<String>, fix: &str) -> InstallerError {
    InstallerError::new(code, msg)
        .with_fix(fix)
        .with_exit(EXIT_REFUSED)
}

/// A refusing finding of `check` as the error the command reports.
pub fn finding_error(f: &cig_compat::Finding) -> InstallerError {
    let mut e = InstallerError::new(f.code, f.headline.clone())
        .with_subject(f.subject.clone())
        .with_exit(EXIT_REFUSED);
    // the error carries one `fix`; #4's other ways out ride in the hint, in its words
    let hint: Vec<String> = f
        .detail
        .iter()
        .cloned()
        .chain(f.fixes.iter().skip(1).cloned())
        .collect();
    if !hint.is_empty() {
        e = e.with_hint(hint.join("; "));
    }
    if let Some(fix) = f.fixes.first() {
        e = e.with_fix(fix_command(fix));
    }
    e
}

pub(super) fn pick_carton<'m>(
    cx: &Cx,
    m: &'m Manifest,
    a: &InstallArgs,
) -> Result<&'m Carton, InstallerError> {
    if let Some(want) = &a.carton {
        let pin = Pin::parse(want)
            .map_err(|e| usage("E1870", e.to_string(), "ciginstaller carton list"))?;
        return m.newest_matching(&pin, None).ok_or_else(|| {
            refused(
                "E1811",
                format!("there is no carton {want} on the {} channel", m.channel),
                "ciginstaller carton list",
            )
        });
    }
    if !matches!(a.what, What::Carton) {
        if let Some(mc) = cx
            .state
            .machine_carton
            .as_deref()
            .and_then(|c| Version::parse(c).ok())
        {
            if let Some(c) = m.carton(&mc) {
                return Ok(c);
            }
        }
    }
    m.latest(false).ok_or_else(|| {
        refused(
            "E1811",
            format!("the {} channel has no carton to install", m.channel),
            "ciginstaller carton list",
        )
    })
}

fn tools_of(c: &Carton, m: &Manifest, a: &InstallArgs) -> Result<Vec<String>, InstallerError> {
    let all: Vec<String> = c
        .products
        .keys()
        .filter(|t| t.as_str() != "ciginstaller")
        .cloned()
        .collect();
    let named = |names: &[String]| -> Result<Vec<String>, InstallerError> {
        for n in names {
            if !all.contains(n) {
                return Err(refused(
                    "E1814",
                    format!("carton {} has no tool called `{n}`", c.version),
                    &format!("ciginstaller carton show {}", c.version),
                ));
            }
        }
        Ok(names.to_vec())
    };
    match &a.what {
        What::Carton => Ok(all),
        What::Stick(t) => named(std::slice::from_ref(t)),
        What::Pick(names) => named(names),
        What::Pack(p) => {
            let members = m.pack(&c.version, p).ok_or_else(|| {
                refused(
                    "E1813",
                    format!("carton {} has no pack called `{p}`", c.version),
                    &format!("ciginstaller carton show {}", c.version),
                )
            })?;
            named(members)
        }
    }
}

fn shell_of(arg: &Option<String>) -> Result<Shell, InstallerError> {
    let name = match arg {
        Some(s) => s.clone(),
        None => std::env::var("SHELL").unwrap_or_default(),
    };
    if name.is_empty() {
        return Ok(Shell::Sh);
    }
    match Shell::from_program(&name) {
        Some(s) => Ok(s),
        None if arg.is_some() => Err(usage(
            "E1870",
            format!("`{name}` is not a shell this installer writes a PATH block for"),
            "ciginstaller install carton --shell bash",
        )),
        None => Ok(Shell::Sh),
    }
}

/// The shells whose profile gets a PATH block. On Windows that is both PowerShells (`pwsh` 7 and
/// Windows PowerShell 5.1, each with its own profile) whatever `--shell` names among them, plus
/// any other shell `--shell` asks for (Git Bash, say); `$SHELL` is not consulted there, because
/// Git Bash sets it for every program it starts. Elsewhere it is the one shell `shell_of` finds.
fn shells_of(arg: &Option<String>) -> Result<Vec<Shell>, InstallerError> {
    if !cfg!(windows) {
        return Ok(vec![shell_of(arg)?]);
    }
    let mut out = Shell::windows_defaults().to_vec();
    if arg.is_some() {
        let s = shell_of(arg)?;
        if !out.contains(&s) {
            out.push(s);
        }
    }
    Ok(out)
}

fn bin_on_path(home: &cigi_home::Home) -> bool {
    let bin = home.bin();
    std::env::var_os("PATH")
        .map(|p| {
            std::env::split_paths(&p).any(|d| {
                d == bin
                    || (cfg!(windows)
                        && cigi_home::path::same_entry(
                            &d.to_string_lossy(),
                            &bin.to_string_lossy(),
                        ))
            })
        })
        .unwrap_or(false)
}

/// The label of a build set: `cig 1.20.0, cigo 1.17.0`.
fn list_of(items: &[(String, Version)]) -> String {
    items
        .iter()
        .map(|(t, v)| format!("{t} {v}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Fetch and admit each build in `todo`, and unpack it into a scratch staging directory that is
/// removed when the `Fetched` drops. Nothing in the home changes (SPEC 12.1 step 3, 12.2 step 5).
pub(super) fn fetch_builds(
    cx: &Cx,
    m: &Manifest,
    todo: &[(String, Version)],
) -> Result<Vec<Fetched>, InstallerError> {
    let _ = sweep_downloads(&cx.home);
    let mut fetched: Vec<Fetched> = Vec::new();
    for (t, v) in todo {
        let art_size = m.artifact(t, v, &cx.target).map(|x| x.size);
        let mut bar = Bar(cx.ui.progress(&format!("fetching {t} {v}"), art_size));
        let staged =
            get_artifact(cx.source.as_ref(), &cx.trust, m, t, v, &cx.home, &mut bar).map_err(ie)?;
        let dir = downloads_dir(&cx.home).join(format!(".stage-{}-{t}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).map_err(|e| {
            InstallerError::new("E1835", format!("cannot create {}: {e}", dir.display()))
        })?;
        let scratch = Scratch(dir.clone());
        let files = stage_artifact(m, &staged, &dir).map_err(ie)?;
        fetched.push(Fetched {
            tool: t.clone(),
            version: v.clone(),
            staged,
            files,
            _scratch: scratch,
        });
    }
    Ok(fetched)
}

/// Make a toolchain of each fetched build inside `txn`, and the humidor mirror for cig.
pub(super) fn materialize_fetched(
    txn: &mut Txn,
    home: &cigi_home::Home,
    m: &Manifest,
    cv: &Version,
    fetched: &[Fetched],
) -> Result<(), InstallerError> {
    for f in fetched {
        let art = &f.staged.art;
        let tooldef = m
            .tool(&f.tool)
            .ok_or_else(|| bug(format!("the manifest lost the definition of {}", f.tool)))?;
        let shims: Vec<ShimRef> = tooldef
            .shims
            .iter()
            .map(|s| ShimRef {
                name: s.name.clone(),
                path: if cfg!(windows) {
                    s.windows_path.clone().unwrap_or_else(|| s.path.clone())
                } else {
                    s.path.clone()
                },
            })
            .collect();
        let prov = Provenance {
            from_carton: Some(cv.to_string()),
            source: ReceiptSource {
                repo: art.repo.clone(),
                tag: art.tag.clone(),
                asset: art.asset.clone(),
                origin: f.staged.origin.clone(),
            },
            artifact: ReceiptArtifact {
                sha256: f.staged.verified.sha256.clone(),
                size: art.size,
            },
            trust: ReceiptTrust {
                level: f.staged.verified.level.as_str().to_string(),
                independent: f.staged.verified.independent,
                attested_by: f.staged.verified.attested_by.clone(),
                manifest_sequence: m.sequence,
            },
            shims,
            license: m
                .product(&f.tool, &f.version)
                .and_then(|p| p.license.clone()),
        };
        materialize(txn, home, &f.tool, &f.version, &f.files, prov).map_err(ie)?;
        if f.tool == "cig" {
            if let Some(bin) = super::tool_exe(home, "cig", &f.version) {
                mirror_humidor(txn, home, &f.version, &bin).map_err(ie)?;
            }
        }
    }
    Ok(())
}

pub fn run(cx: &mut Cx, a: &InstallArgs) -> Result<Report, InstallerError> {
    cx.lock_and_recover()?;
    run_locked(cx, a)
}

/// `install` with the lock already held and `recover` done.
pub fn run_locked(cx: &mut Cx, a: &InstallArgs) -> Result<Report, InstallerError> {
    let m: Manifest = cx.manifest(ManifestMode::Fresh)?.clone();
    let carton = pick_carton(cx, &m, a)?.clone();
    let cv = carton.version.clone();
    let tools = tools_of(&carton, &m, a)?;

    // refusals before a byte is fetched
    if m.is_carton_yanked(&cv) && !a.allow_yanked {
        let why = m
            .yanked
            .iter()
            .find(|y| y.what == cig_compat::Yankable::Carton(cv.clone()))
            .map(|y| y.reason.clone())
            .unwrap_or_default();
        return Err(refused(
            "E1803",
            format!("carton {cv} was yanked: {why}"),
            &format!("ciginstaller install carton --carton {cv} --allow-yanked"),
        ));
    }
    let mut want: Vec<(String, Version)> = Vec::new();
    for t in &tools {
        let v = carton
            .products
            .get(t)
            .cloned()
            .ok_or_else(|| bug(format!("carton {cv} lost its {t}")))?;
        if m.is_product_yanked(t, &v) && !a.allow_yanked {
            return Err(refused(
                "E1803",
                format!("{t} {v} was yanked"),
                &format!("ciginstaller install carton --carton {cv} --allow-yanked"),
            ));
        }
        if m.artifact(t, &v, &cx.target).is_none() {
            return Err(refused(
                "E1815",
                format!("there is no build of {t} {v} for {}", cx.target),
                &format!("ciginstaller carton show {cv}"),
            ));
        }
        want.push((t.clone(), v));
    }

    // the marry-up check of the result
    let machine = cx.state.machine_carton.clone();
    let machine_v = machine.as_deref().and_then(|c| Version::parse(c).ok());
    let on_machine = machine_v.as_ref().is_none_or(|mv| *mv == cv);
    let mut items: Vec<(String, Version)> = Vec::new();
    let base = if on_machine {
        Some(cv.to_string())
    } else {
        machine.clone()
    };
    if let Some(rec) = base.as_deref().and_then(|b| cx.state.cartons.get(b)) {
        for (t, v) in &rec.tools {
            if let Ok(v) = Version::parse(v) {
                items.push((t.clone(), v));
            }
        }
    }
    for (t, v) in &want {
        items.retain(|(x, _)| x != t);
        items.push((t.clone(), v.clone()));
    }
    let origin = match (&machine_v, on_machine) {
        (Some(mv), false) => Origin::Stick { carton: mv.clone() },
        _ => Origin::Carton(cv.clone()),
    };
    let mut warnings: Vec<String> = Vec::new();
    // an advisory is shown in full once per version (#4, #12) and a security one is reminded of
    // every time after; the memory is written with the install's own state save, so a dry run or
    // a failed install leaves it alone
    let mut newly_shown: Vec<cig_compat::Finding> = Vec::new();
    let mut reminded = 0usize;
    for f in check(&m, &Combination { items, origin }, &cx.host) {
        if f.severity == Severity::Refuse {
            return Err(finding_error(&f));
        }
        if crate::advisories::is_advisory(&f) {
            match crate::advisories::triage(&m, &cx.state, &f, a.dry_run) {
                crate::advisories::Triage::Skip => continue,
                crate::advisories::Triage::Remind => {
                    reminded += 1;
                    continue;
                }
                crate::advisories::Triage::Show => newly_shown.push(f.clone()),
            }
        }
        warnings.push(crate::advisories::line(&f));
    }
    if reminded > 0 {
        warnings.push(crate::advisories::reminder(reminded));
    }

    let todo: Vec<(String, Version)> = want
        .iter()
        .filter(|(t, v)| !present(&cx.home, t, v))
        .cloned()
        .collect();
    let recorded = cx.state.cartons.get(&cv.to_string()).is_some_and(|r| {
        want.iter()
            .all(|(t, v)| r.tools.get(t) == Some(&v.to_string()))
    });
    let what_text = match &a.what {
        What::Carton => format!("carton {cv}"),
        What::Pack(p) => format!("pack {p} of carton {cv}"),
        What::Pick(n) => format!("{} of carton {cv}", n.join(", ")),
        What::Stick(t) => format!("stick {t} of carton {cv}"),
    };
    // A stick, pack or pick is for tools that run without a pin: "already installed" also means
    // the machine carton has each of them at this build, or a stick row names it (a record in
    // some other carton is reachable only through a pin).
    let reachable = matches!(a.what, What::Carton)
        || want.iter().all(|(t, v)| {
            let vs = v.to_string();
            let on_machine = machine
                .as_deref()
                .and_then(|m| cx.state.cartons.get(m))
                .is_some_and(|r| r.tools.get(t) == Some(&vs));
            on_machine
                || cx
                    .state
                    .sticks
                    .iter()
                    .any(|s| &s.tool == t && s.version == vs)
        });
    if todo.is_empty() && recorded && reachable && cx.state.machine_carton.is_some() {
        record_answers(cx, a)?;
        return Ok(
            Report::done(format!("{what_text} is already installed")).data(json!({
                "carton": cv.to_string(), "installed": [], "changed": false,
                "first_run": first_run_view(&cx.state),
            })),
        );
    }
    if a.dry_run {
        let mut r = Report::done(format!("dry run: would install {what_text}"));
        for (t, v) in &want {
            let verb = if todo.iter().any(|(x, y)| x == t && y == v) {
                "fetch and verify"
            } else {
                "already here"
            };
            r = r.detail(format!("{t} {v}: {verb}"));
        }
        return Ok(r.data(json!({
            "carton": cv.to_string(), "dry_run": true,
            "tools": want.iter().map(|(t, v)| json!({"tool": t, "version": v.to_string()})).collect::<Vec<_>>()
        })));
    }

    // the licenses are shown (and acknowledged) once, before a byte is fetched; the record is written
    // with the install's own state save, so a decline or a failed install leaves it alone
    let terms_note = super::terms::gate(cx, &m, &cv, &want)?;

    // fetch and admit, outside the transaction: a failure here leaves the home untouched
    let fetched = fetch_builds(cx, &m, &todo)?;

    // the PATH question comes before the transaction
    let shells = shells_of(&a.shell)?;
    let mut add_path = false;
    if !a.no_path {
        let bin = cx.home.bin();
        let mut files: Vec<String> = Vec::new();
        let mut shown: Vec<String> = Vec::new();
        if registry_supported() {
            // the user's PATH in the registry is the route every program started after a sign-in
            // or a new terminal sees; the profile blocks cover PowerShell sessions beyond it
            files.push(cigi_home::path::REGISTRY_PATH.to_string());
            shown.push(registry_line(&bin));
        }
        for sh in &shells {
            let (profile, block) = path_block(*sh, &cx.home);
            files.push(profile.display().to_string());
            if shells.len() > 1 || registry_supported() {
                shown.push(format!("{}:\n{}", profile.display(), block.trim_end()));
            } else {
                shown.push(block.trim_end().to_string());
            }
        }
        let q = Question::yes_no("Add it?", true)
            .summary(&format!(
                "put {} first on PATH in {}",
                bin.display(),
                files.join(" and ")
            ))
            .detail(&shown.join("\n\n"));
        add_path = cx.ui.ask(&q).is_yes(&q);
    }

    if crate::advisories::visible(&cx.ui) {
        crate::advisories::mark(&mut cx.state, &newly_shown.iter().collect::<Vec<_>>());
    }
    let mut txn = cx.begin(&format!("install {what_text}"))?;
    let applied = apply(
        cx,
        &mut txn,
        &Plan {
            m: &m,
            carton: &carton,
            want: &want,
            fetched: &fetched,
            shells: &shells,
            add_path,
            args: a,
        },
    );
    let txn_id = txn.id().to_string();
    let applied = settle(txn, applied)?;

    // the registry PATH comes after the commit: `state.json` already says what this install put
    // there, so a failure here is a finding for `doctor` and `repair`, never a torn transaction
    let registry_result = applied
        .registry_entry
        .as_ref()
        .map(|e| registry_path_add(e));

    let _ = prune_txns(
        &cx.home,
        cx.lock_ref()?,
        std::time::Duration::from_secs(7 * 24 * 3600),
    );
    for f in &fetched {
        let _ = std::fs::remove_file(&f.staged.path);
    }
    cx.state = State::load(&cx.home).map_err(ie)?;

    // the report
    let levels: Vec<TrustLevel> = fetched.iter().map(|f| f.staged.verified.level).collect();
    let mut rep = Report::done(format!("{what_text} installed: {}", list_of(&want)));
    if fetched.is_empty() {
        rep = rep.detail("nothing needed fetching; the files were already here and intact");
    } else {
        let mut parts = Vec::new();
        for lvl in [
            TrustLevel::Pinned,
            TrustLevel::Attested,
            TrustLevel::Checksummed,
        ] {
            let n = levels.iter().filter(|l| **l == lvl).count();
            if n > 0 {
                parts.push(format!("{} {n}", lvl.as_str()));
            }
        }
        let independent = fetched.iter().all(|f| f.staged.verified.independent);
        let from_bundle = cx.source.name().starts_with("bundle:");
        let how = if independent {
            " (two sources agree)"
        } else if from_bundle {
            " (a bundle: one file, so the sources are not independent)"
        } else {
            " (a mirror: the sources are not independent)"
        };
        rep = rep.detail(format!("trust: {}{how}", parts.join(", ")));
        if from_bundle {
            rep = rep.detail("the bundle is unsigned until #9; every build was checked against the manifest inside it");
        }
    }
    for w in &warnings {
        rep = rep.detail(format!("warning {w}"));
    }
    if let Some(n) = &terms_note {
        rep = rep.detail(n.line.clone());
    }
    if let Some(p) = &a.preset {
        rep = rep.detail(format!(
            "preset {p} is recorded; the guard applies it once cig-orchestrator#66 lands"
        ));
    }
    if !matches!(a.what, What::Carton) {
        if let (Some(mc), Some(mrec)) = (
            machine.as_deref().filter(|m| *m != cv.to_string()),
            machine.as_deref().and_then(|m| cx.state.cartons.get(m)),
        ) {
            for (t, v) in &want {
                if mrec.tools.get(t).is_some_and(|b| *b != v.to_string()) {
                    rep = rep.detail(format!(
                        "{t} {v} is recorded, but the machine's carton {mc} has its own {t}, which runs first; pin carton {cv} to use this one"
                    ));
                }
            }
        }
    }
    let mut registry_word = None;
    match &registry_result {
        Some(Ok(RegistryChange::Added)) => {
            registry_word = Some("added");
            rep = rep.detail(format!(
                "{} now has {}",
                cigi_home::path::REGISTRY_PATH,
                cx.home.bin().display()
            ));
        }
        Some(Ok(_)) => {
            registry_word = Some("already-present");
            rep = rep.detail(format!(
                "{} already has {}",
                cigi_home::path::REGISTRY_PATH,
                cx.home.bin().display()
            ));
        }
        Some(Err(e)) => {
            registry_word = Some("failed");
            rep = rep
                .detail(format!(
                    "warning E1845 could not add {} to {}: {e}; the install itself is complete",
                    cx.home.bin().display(),
                    cigi_home::path::REGISTRY_PATH
                ))
                .next("ciginstaller repair");
        }
        None => {}
    }
    for (_, profile, change) in &applied.blocks {
        let note = match change {
            PathChange::Added => Some(format!("PATH block added to {}", profile.display())),
            PathChange::AlreadyPresent => {
                Some(format!("PATH block already in {}", profile.display()))
            }
            PathChange::Edited => Some(format!(
                "{} has a hand-edited cigscript block; left alone",
                profile.display()
            )),
            _ => None,
        };
        if let Some(n) = note {
            rep = rep.detail(n);
        }
    }
    if !bin_on_path(&cx.home) {
        if cfg!(windows) {
            rep = rep.next("open a new terminal (cmd.exe, PowerShell or Windows Terminal) so it sees the new PATH");
        } else {
            rep = rep.next(format!(
                "open a new shell, or run: export PATH=\"{}:$PATH\"",
                cx.home.bin().display()
            ));
        }
    }
    Ok(rep.data(json!({
        "carton": cv.to_string(),
        "changed": true,
        "source": cx.source.name(),
        "preset": a.preset,
        "first_run": first_run_view(&cx.state),
        "terms": terms_note.as_ref().map(|n| n.data.clone()),
        "txn": txn_id,
        "tools": want.iter().map(|(t, v)| json!({
            "tool": t, "version": v.to_string(),
            "trust": fetched.iter().find(|f| &f.tool == t).map(|f| f.staged.verified.level.as_str()),
        })).collect::<Vec<_>>(),
        "path_block": match applied.path() {
            Some(PathChange::Added) => "added",
            Some(PathChange::AlreadyPresent) => "already-present",
            Some(PathChange::Edited) => "edited",
            _ => "skipped",
        },
        "path_registry": registry_word,
    })))
}

struct Plan<'a> {
    m: &'a Manifest,
    carton: &'a Carton,
    want: &'a [(String, Version)],
    fetched: &'a [Fetched],
    shells: &'a [Shell],
    add_path: bool,
    args: &'a InstallArgs,
}

struct Applied {
    /// Each profile a block was asked of, and what happened to it.
    blocks: Vec<(Shell, PathBuf, PathChange)>,
    /// The directory to put on the user's registry PATH once the transaction commits.
    registry_entry: Option<PathBuf>,
}

impl Applied {
    /// One word for the blocks: the first that is not simply there, else `AlreadyPresent`.
    fn path(&self) -> Option<PathChange> {
        let mut out = None;
        for (_, _, c) in &self.blocks {
            match (c, out) {
                (PathChange::Edited, _) => return Some(PathChange::Edited),
                (PathChange::Added, _) => out = Some(PathChange::Added),
                (PathChange::AlreadyPresent, None) => out = Some(PathChange::AlreadyPresent),
                _ => {}
            }
        }
        out
    }
}

/// The ciginstaller toolchain: the running binary and the shim beside it, put in the store so
/// `bin/ciginstaller`, `bin/cigi` and every shim are links of store objects. Returns the two
/// objects `(ciginstaller, cigi-shim)`.
fn bootstrap(cx: &Cx, txn: &mut Txn) -> Result<(ObjectId, ObjectId), InstallerError> {
    let ver = Version::parse(env!("CARGO_PKG_VERSION"))
        .map_err(|e| bug(format!("the crate version does not parse: {e}")))?;
    let objects = |home: &cigi_home::Home| -> Option<(ObjectId, ObjectId)> {
        if !present(home, "ciginstaller", &ver) {
            return None;
        }
        let r = read_receipt(home, "ciginstaller", &ver).ok()?;
        let id = |path: &str| {
            r.files
                .iter()
                .find(|f| f.path == path)
                .map(|f| ObjectId::new(f.sha256.clone(), f.exec))
        };
        Some((id("bin/ciginstaller")?, id("bin/cigi-shim")?))
    };
    if let Some(found) = objects(&cx.home) {
        return Ok(found);
    }
    let exe =
        std::env::current_exe().map_err(|e| bug(format!("cannot find the running binary: {e}")))?;
    let shim = exe.with_file_name(format!("cigi-shim{}", std::env::consts::EXE_SUFFIX));
    if !shim.is_file() {
        return Err(InstallerError::new(
            "E1840",
            format!("cigi-shim is not next to {}", exe.display()),
        )
        .with_hint("the installer needs its shim binary beside it to make the tools' shims")
        .with_fix("reinstall ciginstaller from https://github.com/otmof-ops/cig-installer")
        .with_exit(cigi_ui::EXIT_FAILED));
    }
    let (sha, size) = {
        let bytes =
            std::fs::read(&exe).map_err(|e| bug(format!("cannot read {}: {e}", exe.display())))?;
        (cigi_digest(&bytes), bytes.len() as u64)
    };
    let files = vec![
        StagedFile {
            path: PathBuf::from("bin/ciginstaller"),
            source: exe,
            exec: true,
        },
        StagedFile {
            path: PathBuf::from("bin/cigi-shim"),
            source: shim,
            exec: true,
        },
    ];
    let prov = Provenance {
        from_carton: None,
        source: ReceiptSource {
            repo: "otmof-ops/cig-installer".into(),
            tag: format!("v{ver}"),
            asset: "ciginstaller".into(),
            origin: "bootstrap".into(),
        },
        artifact: ReceiptArtifact { sha256: sha, size },
        trust: ReceiptTrust {
            level: "bootstrap".into(),
            independent: false,
            attested_by: None,
            manifest_sequence: 0,
        },
        shims: Vec::new(),
        license: Some(super::terms::INSTALLER_LICENSE.to_string()),
    };
    materialize(txn, &cx.home, "ciginstaller", &ver, &files, prov).map_err(ie)?;
    objects(&cx.home)
        .ok_or_else(|| bug("the ciginstaller toolchain does not verify right after it was made"))
}

fn cigi_digest(bytes: &[u8]) -> String {
    cigi_source::digest::sha256_hex(bytes)
}

fn apply(cx: &mut Cx, txn: &mut Txn, p: &Plan<'_>) -> Result<Applied, InstallerError> {
    let home = cx.home.clone();
    let cv = p.carton.version.clone();
    let now = now_rfc3339();
    txn.mkdir_all(&home.toolchains()).map_err(ie)?;

    // 1. toolchains, the humidor mirror for cig
    materialize_fetched(txn, &home, p.m, &cv, p.fetched)?;

    // 2. the installer's own toolchain and links
    let (exe_obj, shim_obj) = bootstrap(cx, txn)?;

    // 2b. the install's own settings, inside the same transaction (a rollback undoes them)
    // the file is read again here, under the lock: what this edits is the file as it is now,
    // not the copy read when the command started
    if p.args.preset.is_some() || p.args.reports.is_some() {
        cx.cfg.reread(&cx.home).map_err(ie)?;
    }
    for (key, value) in [("preset", &p.args.preset), ("reports", &p.args.reports)] {
        if let Some(v) = value {
            cx.cfg.set_in(txn, key, v).map_err(ie)?;
        }
    }

    // 3. the new state
    let mut state = cx.state.clone();
    if let Some(fr) = &p.args.first_run {
        record_first_run(&mut state, fr);
    }
    let cvs = cv.to_string();
    let all_products: BTreeSet<&String> = p
        .carton
        .products
        .keys()
        .filter(|t| t.as_str() != "ciginstaller")
        .collect();
    let reason = p.args.reason.clone().unwrap_or_else(|| match &p.args.what {
        What::Carton => "install carton".to_string(),
        What::Pack(n) => format!("install pack {n}"),
        What::Pick(n) => format!("install {}", n.join(", ")),
        What::Stick(t) => format!("install stick {t}"),
    });
    let rec = state
        .cartons
        .entry(cvs.clone())
        .or_insert_with(|| CartonRecord {
            installed_at: now.clone(),
            tools: Default::default(),
            reason,
            last_used: None,
            partial: false,
        });
    for (t, v) in p.want {
        rec.tools.insert(t.clone(), v.to_string());
    }
    rec.partial = !all_products.iter().all(|t| rec.tools.contains_key(*t));
    let old_machine = state.machine_carton.clone();
    if state.machine_carton.is_none() {
        state.machine_carton = Some(cvs.clone());
    } else if !matches!(p.args.what, What::Carton) && old_machine.as_deref() != Some(cvs.as_str()) {
        // a stick for a tool the machine's carton lacks, or has at another build: the row
        // records what the user asked for (the machine's own build still runs first)
        let has = |t: &str, v: &Version| {
            old_machine
                .as_deref()
                .and_then(|m| state.cartons.get(m))
                .is_some_and(|r| r.tools.get(t) == Some(&v.to_string()))
        };
        for (t, v) in p.want {
            if !has(t, v) {
                state.sticks.retain(|s| &s.tool != t);
                state.sticks.push(Stick {
                    tool: t.clone(),
                    version: v.to_string(),
                    from_carton: cvs.clone(),
                    installed_at: now.clone(),
                    reason: format!("install stick {t}"),
                });
            }
        }
    }
    state.channel = cx.channel();
    state.installer.version = env!("CARGO_PKG_VERSION").to_string();
    let lowest = p
        .fetched
        .iter()
        .map(|f| f.staged.verified.level)
        .min()
        .map(|l| l.as_str().to_string())
        .or_else(|| state.manifest.as_ref().map(|m| m.trust_level.clone()))
        .unwrap_or_else(|| "checksummed".to_string());
    state.manifest = Some(ManifestInfo {
        sequence: p.m.sequence,
        channel: p.m.channel.clone(),
        fetched_at: now.clone(),
        expires: p.m.expires.clone(),
        trust_level: lowest,
    });
    cx.record_manifest_digest(&mut state, &p.m.channel, p.m.sequence);
    state.push_history(HistoryEntry {
        id: txn.id().to_string(),
        at: now,
        what: "install".into(),
        from: old_machine,
        to: Some(cvs),
        txn: txn.id().to_string(),
        ok: true,
    });

    // 4. shims: every tool the state now lists (and on Windows `cigi`, which is a shim there),
    //    then the installer's own names
    let names = super::switch::shim_names(&home, &state);
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    sync_shims(txn, &home, &refs, &shim_obj).map_err(ie)?;
    let store = home.object_store();
    for n in super::switch::installer_names() {
        txn.link(&store.object_path(&exe_obj), &shim_path(&home, n))
            .map_err(ie)?;
    }

    // 5. PATH block, in every profile asked of; and, on Windows, the user PATH in the registry,
    //    which is written after the commit and only recorded here
    let mut blocks: Vec<(Shell, PathBuf, PathChange)> = Vec::new();
    if p.add_path {
        for sh in p.shells {
            let (profile, _) = path_block(*sh, &home);
            let change = add_path_block(txn, *sh, &home).map_err(ie)?;
            blocks.push((*sh, profile, change));
        }
    }

    // what the user chose about PATH, so `doctor` and `repair` can tell a block that went missing
    // from one that was never asked for (`--no-path`)
    let done: Vec<Shell> = blocks
        .iter()
        .filter(|(_, _, c)| matches!(c, PathChange::Added | PathChange::AlreadyPresent))
        .map(|(s, _, _)| *s)
        .collect();
    if !done.is_empty() {
        state
            .extra
            .insert("path_block".to_string(), blocks_record(&done));
    }
    let mut registry_entry = None;
    if p.add_path && registry_supported() {
        let entry = home.bin();
        // "preexisting" = on the user's PATH before this install, so not the installer's to
        // remove; a rerun keeps what the first install recorded
        let preexisting = match recorded_registry(&state).filter(|r| r.entry == entry) {
            Some(r) => r.preexisting,
            None => registry_path_has(&entry).unwrap_or(false),
        };
        state.extra.insert(
            cigi_home::path::PATH_REGISTRY_KEY.to_string(),
            registry_record(&entry, preexisting),
        );
        registry_entry = Some(entry);
    }

    // 6. state, flip
    save_and_flip(txn, &home, &state, super::yanked_in(p.m))?;
    Ok(Applied {
        blocks,
        registry_entry,
    })
}

#[allow(dead_code)]
fn _keep(_: &Path) {
    let _ = platform::exe_suffix;
    let _ = Icon::Done;
}
