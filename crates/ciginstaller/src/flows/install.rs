// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0
//! `install` (SPEC 12.1): resolve the target from one carton, fetch and admit what is missing,
//! then one transaction: toolchains, humidor mirror, shims, PATH block, state, the index flip.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use cig_compat::{
    check, fix_command, Carton, Combination, Manifest, Origin, Pin, Severity, Version,
};
use cigi_home::state::{CartonRecord, HistoryEntry, ManifestInfo, Stick};
use cigi_home::{
    add_path_block, materialize, mirror_humidor, now_rfc3339, path_block, platform, prune_txns,
    read_receipt, shim_path, sync_shims, ObjectId, PathChange, Provenance, ReceiptArtifact,
    ReceiptSource, ReceiptTrust, Shell, ShimRef, StagedFile, State, Txn,
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
    /// Why this install happens (`install carton`, `pin fetch`), recorded in `state.json`.
    pub reason: Option<String>,
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
            reason: None,
        }
    }
}

/// Adapts the UI's bar to the source crate's progress trait.
struct Bar(Box<dyn cigi_ui::Progress>);

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
struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Fetched {
    tool: String,
    version: Version,
    staged: StagedArtifact,
    files: Vec<StagedFile>,
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
    if !f.detail.is_empty() {
        e = e.with_hint(f.detail.join("; "));
    }
    if let Some(fix) = f.fixes.first() {
        e = e.with_fix(fix_command(fix));
    }
    e
}

fn pick_carton<'m>(
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

fn bin_on_path(home: &cigi_home::Home) -> bool {
    let bin = home.bin();
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d == bin))
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
    for f in check(&m, &Combination { items, origin }, &cx.host) {
        if f.severity == Severity::Refuse {
            return Err(finding_error(&f));
        }
        warnings.push(format!("{}: {}", f.code, f.headline));
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
    if todo.is_empty() && recorded && cx.state.machine_carton.is_some() {
        return Ok(Report::done(format!("{what_text} is already installed"))
            .data(json!({ "carton": cv.to_string(), "installed": [], "changed": false })));
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

    // fetch and admit, outside the transaction: a failure here leaves the home untouched
    let _ = sweep_downloads(&cx.home);
    let mut fetched: Vec<Fetched> = Vec::new();
    for (t, v) in &todo {
        let art_size = m.artifact(t, v, &cx.target).map(|x| x.size);
        let mut bar = Bar(cx.ui.progress(&format!("fetching {t} {v}"), art_size));
        let staged = get_artifact(cx.source.as_ref(), &cx.trust, &m, t, v, &cx.home, &mut bar)
            .map_err(ie)?;
        let dir = downloads_dir(&cx.home).join(format!(".stage-{}-{t}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).map_err(|e| {
            InstallerError::new("E1835", format!("cannot create {}: {e}", dir.display()))
        })?;
        let scratch = Scratch(dir.clone());
        let files = stage_artifact(&m, &staged, &dir).map_err(ie)?;
        fetched.push(Fetched {
            tool: t.clone(),
            version: v.clone(),
            staged,
            files,
            _scratch: scratch,
        });
    }

    // the PATH question comes before the transaction
    let shell = shell_of(&a.shell)?;
    let mut add_path = false;
    if !a.no_path {
        let (profile, block) = path_block(shell, &cx.home);
        let q = Question::yes_no("Add it?", true)
            .summary(&format!(
                "put {} first on PATH in {}",
                cx.home.bin().display(),
                profile.display()
            ))
            .detail(block.trim_end());
        add_path = cx.ui.ask(&q).is_yes(&q);
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
            shell,
            add_path,
            args: a,
        },
    );
    let txn_id = txn.id().to_string();
    let applied = settle(txn, applied)?;

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
        let how = if independent {
            " (two sources agree)"
        } else {
            " (a mirror: the sources are not independent)"
        };
        rep = rep.detail(format!("trust: {}{how}", parts.join(", ")));
    }
    for w in &warnings {
        rep = rep.detail(format!("warning {w}"));
    }
    let path_note = match applied.path {
        Some(PathChange::Added) => {
            Some(format!("PATH block added to {}", applied.profile.display()))
        }
        Some(PathChange::AlreadyPresent) => Some(format!(
            "PATH block already in {}",
            applied.profile.display()
        )),
        Some(PathChange::Edited) => Some(format!(
            "{} has a hand-edited cigscript block; left alone",
            applied.profile.display()
        )),
        _ => None,
    };
    if let Some(n) = &path_note {
        rep = rep.detail(n.clone());
    }
    if !bin_on_path(&cx.home) {
        rep = rep.next(format!(
            "open a new shell, or run: export PATH=\"{}:$PATH\"",
            cx.home.bin().display()
        ));
    }
    Ok(rep.data(json!({
        "carton": cv.to_string(),
        "changed": true,
        "txn": txn_id,
        "tools": want.iter().map(|(t, v)| json!({
            "tool": t, "version": v.to_string(),
            "trust": fetched.iter().find(|f| &f.tool == t).map(|f| f.staged.verified.level.as_str()),
        })).collect::<Vec<_>>(),
        "path_block": match applied.path {
            Some(PathChange::Added) => "added",
            Some(PathChange::AlreadyPresent) => "already-present",
            Some(PathChange::Edited) => "edited",
            _ => "skipped",
        },
    })))
}

struct Plan<'a> {
    m: &'a Manifest,
    carton: &'a Carton,
    want: &'a [(String, Version)],
    fetched: &'a [Fetched],
    shell: Shell,
    add_path: bool,
    args: &'a InstallArgs,
}

struct Applied {
    path: Option<PathChange>,
    profile: PathBuf,
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
    for f in p.fetched {
        let art = &f.staged.art;
        let tooldef =
            p.m.tool(&f.tool)
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
                manifest_sequence: p.m.sequence,
            },
            shims,
        };
        materialize(txn, &home, &f.tool, &f.version, &f.files, prov).map_err(ie)?;
        if f.tool == "cig" {
            if let Some(bin) = super::tool_exe(&home, "cig", &f.version) {
                mirror_humidor(txn, &home, &f.version, &bin).map_err(ie)?;
            }
        }
    }

    // 2. the installer's own toolchain and links
    let (exe_obj, shim_obj) = bootstrap(cx, txn)?;

    // 3. the new state
    let mut state = cx.state.clone();
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
        // a stick for a tool the machine's carton lacks
        let has = |t: &str| {
            old_machine
                .as_deref()
                .and_then(|m| state.cartons.get(m))
                .is_some_and(|r| r.tools.contains_key(t))
        };
        for (t, v) in p.want {
            if !has(t) {
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
    if let Some(meta) = cigi_source::fetch::read_fetched(&home, &p.m.channel) {
        let entry = state
            .extra
            .entry(crate::cx::DIGESTS_KEY.to_string())
            .or_insert_with(|| json!({}));
        if let Some(o) = entry.as_object_mut() {
            o.insert(
                crate::cx::digest_key(&p.m.channel, p.m.sequence),
                json!(meta.sha256),
            );
        }
    }
    state.push_history(HistoryEntry {
        id: txn.id().to_string(),
        at: now,
        what: "install".into(),
        from: old_machine,
        to: Some(cvs),
        txn: txn.id().to_string(),
        ok: true,
    });

    // 4. shims: every tool the state now lists, plus the installer's own two names
    let mut names: BTreeSet<String> = BTreeSet::new();
    let mut builds: Vec<(String, String)> = Vec::new();
    for r in state.cartons.values() {
        builds.extend(r.tools.iter().map(|(t, v)| (t.clone(), v.clone())));
    }
    builds.extend(
        state
            .sticks
            .iter()
            .map(|s| (s.tool.clone(), s.version.clone())),
    );
    for (t, v) in &builds {
        if let Ok(v) = Version::parse(v) {
            if let Ok(r) = read_receipt(&home, t, &v) {
                names.extend(r.shims.iter().map(|s| s.name.clone()));
            }
        }
    }
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    sync_shims(txn, &home, &refs, &shim_obj).map_err(ie)?;
    let store = home.object_store();
    for n in ["ciginstaller", "cigi"] {
        txn.link(&store.object_path(&exe_obj), &shim_path(&home, n))
            .map_err(ie)?;
    }

    // 5. PATH block
    let (profile, _) = path_block(p.shell, &home);
    let path = if p.add_path {
        Some(add_path_block(txn, p.shell, &home).map_err(ie)?)
    } else {
        None
    };

    // 6. state, flip
    save_and_flip(txn, &home, &state)?;
    Ok(Applied { path, profile })
}

#[allow(dead_code)]
fn _keep(_: &Path) {
    let _ = platform::exe_suffix;
    let _ = Icon::Done;
}
