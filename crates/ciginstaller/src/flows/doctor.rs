// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `doctor` (SPEC 12.5, #13): one report for the whole family. Read-only: it takes no lock and
//! writes nothing, and it asks the network only under `--network`. Every finding says what was
//! found, what was checked to find it, and the one command that fixes it.
//!
//! Not in this build, so said to be absent rather than faked: the guard's checks and the ashtray's
//! queue and consent. The guard's kernel features (user namespaces, BPF LSM, the kernel's version)
//! are read from `/proc` and `/sys`, and the tin's disk use from `<home>/tin`. The tools' own
//! checks run by default: each installed product is asked `<tool> __doctor --json` (never
//! `<tool> doctor`, which for cig and cigo is a stub that comes back here), and the findings it
//! answers with are shown as the installer's own, under the tool's heading (docs/decisions/0003).
//! `--tools` shows those and nothing else.
//!
//! The one test seam: `CIGI_DOCTOR_PROBE_ROOT=<dir>` makes the kernel probes read `<dir>/proc/...`
//! and `<dir>/sys/...` instead of the host's. It changes where doctor reads from and nothing else:
//! doctor writes nothing, so there is nothing for it to redirect, and it never makes a finding
//! pass that the folder's files do not say.

use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cig_compat::{check, Combination, Host, Manifest, Origin, Severity, Version};
use cigi_home::path::block_state;
use cigi_home::platform;
use cigi_home::uninstall::leftover_asides;
use cigi_home::{
    installed, list_txns, read_receipt, recorded_shells, verify_toolchain, Damage, PathChange,
    Shell, TxnStatus,
};
use cigi_ui::{Icon, InstallerError, Report};
use serde_json::json;

use super::gc::{self, PENDING_KEY};
use super::health::{
    builds, humidor_bad, index_state, missing_shims, shim_names, size_text, IndexState,
};
use crate::cx::{now_override, Cx};

#[derive(Debug, Clone, Default)]
pub struct DoctorArgs {
    /// Fetch the manifest now (the only thing that touches the network).
    pub network: bool,
    /// Show only the installed tools' own `__doctor --json` findings, each under its heading.
    /// Without it, doctor shows the installer's findings and then the tools' as well.
    pub tools: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Sev {
    Ok,
    Note,
    Action,
}

impl Sev {
    fn word(self) -> &'static str {
        match self {
            Sev::Ok => "ok",
            Sev::Note => "note",
            Sev::Action => "action",
        }
    }
}

/// One finding: "I think `found`, because I checked `because`", and what fixes it.
#[derive(Debug, Clone)]
pub struct Item {
    pub section: &'static str,
    pub sev: Sev,
    pub found: String,
    pub because: String,
    pub fix: Option<String>,
    pub code: Option<&'static str>,
}

impl Item {
    fn new(
        section: &'static str,
        sev: Sev,
        found: impl Into<String>,
        because: impl Into<String>,
    ) -> Item {
        Item {
            section,
            sev,
            found: found.into(),
            because: because.into(),
            fix: None,
            code: None,
        }
    }
    fn fix(mut self, cmd: impl Into<String>) -> Item {
        self.fix = Some(cmd.into());
        self
    }
    fn code(mut self, c: &'static str) -> Item {
        self.code = Some(c);
        self
    }
    fn json(&self) -> serde_json::Value {
        json!({
            "section": self.section,
            "severity": self.sev.word(),
            "found": self.found,
            "because": self.because,
            "fix": self.fix,
            "code": self.code,
        })
    }
}

fn ok(s: &'static str, found: impl Into<String>, because: impl Into<String>) -> Item {
    Item::new(s, Sev::Ok, found, because)
}
fn note(s: &'static str, found: impl Into<String>, because: impl Into<String>) -> Item {
    Item::new(s, Sev::Note, found, because)
}
fn act(s: &'static str, found: impl Into<String>, because: impl Into<String>) -> Item {
    Item::new(s, Sev::Action, found, because)
}

fn now_epoch() -> i64 {
    now_override().unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64)
    })
}

/// Everything doctor looks at, as findings, in the order they are shown. No lock, no writes.
pub fn findings(cx: &mut Cx, a: &DoctorArgs) -> Vec<Item> {
    let mut v: Vec<Item> = Vec::new();
    let home = cx.home.clone();
    let any = cx.state.machine_carton.is_some()
        || !cx.state.cartons.is_empty()
        || !installed(&home).is_empty();

    leftovers(cx, &mut v);
    if !any && !home.installer().exists() {
        v.push(
            note(
                "install",
                "nothing is installed here",
                format!(
                    "{} has no state.json and no toolchain",
                    home.root().display()
                ),
            )
            .fix("ciginstaller install carton"),
        );
        return v;
    }
    layout(cx, &mut v);
    transactions(cx, &mut v);
    toolchains(cx, &mut v);
    index(cx, &mut v);
    shims(cx, &mut v);
    moved_home(cx, &mut v);
    home_anchor(cx, &mut v);
    path_order(cx, &mut v);
    path_block_item(cx, &mut v);
    matrix(cx, a, &mut v);
    programs_item(cx, &mut v);
    freshness(cx, &mut v);
    disk(cx, &mut v);
    tin_disk(cx, &mut v);
    humidor_item(cx, &mut v);
    reports_items(cx, &mut v);
    mirror_items(cx, &mut v);
    guard_kernel_features(&mut v);
    v.push(note(
        "not in this build",
        "the guard's own checks are not checked",
        "this build of ciginstaller has no guard checks of its own, so there is nothing to look at",
    ));
    v
}

/// A mirror reached over plain http (#48): off loopback it is refused outright, on loopback it
/// works for builds a pin vouches for and no others. Reads the setting only.
fn mirror_items(cx: &Cx, v: &mut Vec<Item>) {
    use cigi_home::config::{loopback_host, plain_http_host};
    let mirror = cx.cfg.get("mirror");
    let Some(host) = plain_http_host(&mirror) else {
        return;
    };
    let because = "the `mirror` setting (or CIGI_MIRROR)";
    if loopback_host(&host) {
        v.push(note(
            "mirror",
            format!("{mirror} is plain http on this machine: only builds a pin vouches for are accepted from it"),
            because,
        ));
    } else {
        v.push(
            act(
                "mirror",
                format!("{mirror} is plain http, which every fetch refuses"),
                because,
            )
            .fix("ciginstaller config unset mirror")
            .code("E1900"),
        );
    }
}

/// The ashtray's queue, the consent setting and each report route's last success and last
/// failure (#13, #28 section 4). Reads two small files; asks nobody anything.
fn reports_items(cx: &Cx, v: &mut Vec<Item>) {
    let root = cx.home.root();
    let (consent, _) = crate::crash::consent(&cx.cfg);
    let consent = match consent {
        crate::crash::Consent::Ask => "ask",
        crate::crash::Consent::Always => "always",
        crate::crash::Consent::Never => "never",
    };
    let Ok(l) = crate::crash::list(&root) else {
        return;
    };
    let queued = l.reports.iter().filter(|r| !r.filed()).count();
    let failing = l
        .reports
        .iter()
        .filter(|r| !r.filed() && r.queue.last_error.is_some())
        .count();
    let found =
        format!("{queued} report(s) queued, {failing} with a failed attempt; reports = {consent}");
    let because = "the ashtray's reports and the `reports` setting";
    v.push(if failing > 0 {
        act("reports", found, because)
            .fix("ciginstaller crash test-route")
            .code("E1857")
    } else if queued > 0 {
        note("reports", found, because).fix("ciginstaller crash status")
    } else {
        ok("reports", found, because)
    });
    for (route, log) in crate::crash::health::load(&root) {
        v.push(note(
            "reports",
            format!(
                "route {route}: last success {}, last failure {}{}",
                log.last_success.as_deref().unwrap_or("never"),
                log.last_failure.as_deref().unwrap_or("never"),
                log.last_error
                    .as_deref()
                    .map(|e| format!(" ({})", crate::crash::truncate(e, 100)))
                    .unwrap_or_default()
            ),
            "ashtray/.routes.json, written by `crash send` and `crash test-route`",
        ));
    }
}

fn leftovers(cx: &Cx, v: &mut Vec<Item>) {
    for a in leftover_asides(&cx.home) {
        if a.kept {
            continue;
        }
        v.push(
            act(
                "uninstall",
                format!(
                    "an `uninstall --all` removed `installer/` and left its journal at {}",
                    a.path.display()
                ),
                "the sibling of the home that `uninstall --all` moves the journal to",
            )
            .fix("ciginstaller gc")
            .code("E1865"),
        );
    }
}

fn layout(cx: &Cx, v: &mut Vec<Item>) {
    for (dir, mode) in cx.home.loose_dirs() {
        v.push(
            act(
                "home",
                format!(
                    "{} is readable by others (mode {mode:03o}); the home and installer/ are yours alone, mode 700",
                    dir.display()
                ),
                "the mode of the home and of installer/, which hold your settings and journals",
            )
            .fix("ciginstaller repair")
            .code("E1835"),
        );
    }
    match cx.home.check_permissions() {
        Ok(()) => v.push(ok(
            "home",
            "the directories the installer writes into are yours alone",
            "owner and mode of bin/, toolchains/, humidor/store/ and installer/",
        )),
        Err(e) => v.push(
            act(
                "home",
                e.to_string(),
                "owner and mode of the home's directories",
            )
            .fix("ciginstaller explain E1835")
            .code("E1835"),
        ),
    }
}

/// The pid a transaction's `begin` line recorded.
fn txn_pid(home: &cigi_home::Home, id: &str) -> Option<u32> {
    let text = std::fs::read_to_string(home.txn_root().join(id).join("journal.jsonl")).ok()?;
    let first = text.lines().next()?;
    let v: serde_json::Value = serde_json::from_str(first).ok()?;
    v.get("pid")?.as_u64().map(|p| p as u32)
}

fn pid_alive(pid: u32) -> bool {
    pid != std::process::id()
        && cfg!(target_os = "linux")
        && Path::new("/proc").join(pid.to_string()).exists()
}

fn transactions(cx: &Cx, v: &mut Vec<Item>) {
    let txns = list_txns(&cx.home);
    let mut bad = 0;
    for t in &txns {
        match t.status {
            TxnStatus::Open => {
                bad += 1;
                match txn_pid(&cx.home, &t.id).filter(|p| pid_alive(*p)) {
                    Some(pid) => v.push(note(
                        "transactions",
                        format!(
                            "`{}` ({}) is in flight in ciginstaller pid {pid}; it is not interrupted",
                            t.label, t.id
                        ),
                        "the journal has no end line and its process is still running",
                    )),
                    None => v.push(
                        act(
                            "transactions",
                            format!("an interrupted `{}` ({}) is waiting to be settled", t.label, t.id),
                            "the journal in installer/txn has no commit or rollback line and its process is gone",
                        )
                        .fix("ciginstaller repair")
                        .code("E1832"),
                    ),
                }
            }
            TxnStatus::Unreadable => {
                bad += 1;
                v.push(
                    act(
                        "transactions",
                        format!("the journal {} cannot be read", t.id),
                        "the journal's lines in installer/txn",
                    )
                    .fix("ciginstaller explain E1891")
                    .code("E1891"),
                );
            }
            _ => {}
        }
    }
    if bad == 0 {
        v.push(ok(
            "transactions",
            "no transaction is interrupted",
            format!("{} journal(s) in installer/txn, all ended", txns.len()),
        ));
    }
}

fn toolchains(cx: &Cx, v: &mut Vec<Item>) {
    let home = &cx.home;
    let have = installed(home);
    let mut bad = 0;
    for (t, ver) in &have {
        let damage = verify_toolchain(home, t, ver);
        if damage.is_empty() {
            continue;
        }
        bad += 1;
        let words: Vec<String> = damage
            .iter()
            .map(|d| match d {
                Damage::NoReceipt => "has no readable receipt".to_string(),
                Damage::Missing { path } => format!("{path} is missing"),
                Damage::Changed { path, .. } => format!("{path} was changed"),
                Damage::Mode { path } => format!("{path} has the wrong mode"),
                Damage::Extra { path } => format!("{path} is not in the receipt"),
            })
            .collect();
        v.push(
            act(
                "toolchains",
                format!("{t} {ver} differs from its receipt: {}", words.join("; ")),
                format!("every file of toolchains/{t}/{ver} against .cigi-receipt.json"),
            )
            .fix("ciginstaller repair")
            .code("E1831"),
        );
    }
    let have_set: BTreeSet<(String, String)> = have
        .iter()
        .map(|(t, v)| (t.clone(), v.to_string()))
        .collect();
    for (t, ver) in builds(&cx.state) {
        let canon = Version::parse(&ver).map_or(ver.clone(), |p| p.to_string());
        if have_set.contains(&(t.clone(), canon)) {
            continue;
        }
        bad += 1;
        let carton = cx
            .state
            .cartons
            .iter()
            .find(|(_, r)| r.tools.get(&t) == Some(&ver))
            .map(|(c, _)| c.clone())
            .or_else(|| {
                cx.state
                    .sticks
                    .iter()
                    .find(|s| s.tool == t && s.version == ver)
                    .map(|s| s.from_carton.clone())
            });
        let _ = carton;
        // repair builds it again from the manifest's artifact, with a fresh receipt
        v.push(
            act(
                "toolchains",
                format!(
                    "state.json lists {t} {ver} but there is no toolchain with a readable receipt for it"
                ),
                format!("toolchains/{t}/{ver} and its .cigi-receipt.json"),
            )
            .fix(format!("ciginstaller repair --tool {t}"))
            .code("E1831"),
        );
    }
    if cx
        .state
        .extra
        .get(PENDING_KEY)
        .is_some_and(|p| p.as_array().is_some_and(|a| !a.is_empty()))
    {
        v.push(
            note(
                "toolchains",
                "a change removed toolchains from use and was cut off before it deleted them",
                "state.json's pending deletes",
            )
            .fix("ciginstaller gc"),
        );
    }
    if bad == 0 {
        v.push(ok(
            "toolchains",
            format!("{} toolchain(s) match their receipts", have.len()),
            "every file's hash, mode and presence against .cigi-receipt.json",
        ));
    }
}

fn index(cx: &Cx, v: &mut Vec<Item>) {
    if cx.state.machine_carton.is_none() && cx.state.cartons.is_empty() {
        return;
    }
    match index_state(&cx.home, &cx.state, super::cached_yanked(cx)) {
        IndexState::Agrees => v.push(ok(
            "index",
            "state.json and the shim-index agree",
            "the shim-index against the one state.json calls for",
        )),
        IndexState::Differs => v.push(
            act(
                "index",
                "the shim-index does not match state.json (it was edited, or one of the two is stale)",
                "installer/shim-index against what state.json produces",
            )
            .fix("ciginstaller shims sync"),
        ),
        IndexState::Missing => v.push(
            act("index", "the shim-index is missing", "installer/shim-index")
                .fix("ciginstaller shims sync"),
        ),
    }
}

fn shims(cx: &Cx, v: &mut Vec<Item>) {
    if cx.state.machine_carton.is_none() && cx.state.cartons.is_empty() {
        return;
    }
    match missing_shims(&cx.home, &cx.state) {
        None => v.push(note(
            "shims",
            "cannot tell which file the shims should be",
            "there is no readable receipt for ciginstaller's own toolchain",
        )),
        Some(missing) if missing.is_empty() => v.push(ok(
            "shims",
            format!(
                "{} shim(s) are in bin/",
                shim_names(&cx.home, &cx.state).len() + 2
            ),
            "each name in bin/ is the shim object, or the installer's own binary",
        )),
        Some(missing) => v.push(
            act(
                "shims",
                format!("missing or replaced in bin/: {}", missing.join(", ")),
                "each shim in bin/ against the store object it must be a link of",
            )
            .fix("ciginstaller repair")
            .code("E1844"),
        ),
    }
}

fn is_exe(p: &Path) -> bool {
    match std::fs::metadata(p) {
        Ok(m) if m.is_file() => platform::mode_of(p).is_none_or(|mode| mode & 0o111 != 0),
        _ => false,
    }
}

fn same_dir(a: &Path, b: &Path) -> bool {
    a == b
        || match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
            (Ok(x), Ok(y)) => x == y,
            _ => false,
        }
}

fn export_line(bin: &Path) -> String {
    format!("export PATH=\"{}:$PATH\"", bin.display())
}

fn path_order(cx: &Cx, v: &mut Vec<Item>) {
    let home = &cx.home;
    if cx.state.machine_carton.is_none() && cx.state.cartons.is_empty() {
        return;
    }
    let bin = home.bin();
    let dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    let Some(at) = dirs.iter().position(|d| same_dir(d, &bin)) else {
        v.push(
            act(
                "PATH",
                format!("{} is not on PATH in this shell", bin.display()),
                "the directories of $PATH, in order",
            )
            .fix(export_line(&bin))
            .code("E1845"),
        );
        return;
    };
    let mut shadowed: Vec<String> = Vec::new();
    for name in shim_names(home, &cx.state) {
        let file = format!("{name}{}", platform::exe_suffix());
        for d in &dirs[..at] {
            if is_exe(&d.join(&file)) {
                shadowed.push(format!("{name} in {}", d.display()));
                break;
            }
        }
    }
    if shadowed.is_empty() {
        v.push(ok(
            "PATH",
            "the shims come first on PATH in this shell, and no other copy shadows them",
            format!(
                "the {} directories of $PATH, in order, for each family tool",
                dirs.len()
            ),
        ));
    } else {
        v.push(
            act(
                "PATH",
                format!(
                    "another copy comes before the shim: {}",
                    shadowed.join("; ")
                ),
                "the directories of $PATH, in order, for each family tool",
            )
            .fix(export_line(&bin))
            .code("E1844"),
        );
    }
}

fn path_block_item(cx: &Cx, v: &mut Vec<Item>) {
    let home = &cx.home;
    if cx.state.machine_carton.is_none() && cx.state.cartons.is_empty() {
        return;
    }
    let recorded = recorded_shells(&cx.state);
    let shells: Vec<Shell> = if !recorded.is_empty() {
        recorded.clone()
    } else if cfg!(windows) {
        Shell::windows_defaults().to_vec()
    } else {
        vec![std::env::var("SHELL")
            .ok()
            .and_then(|s| Shell::from_program(&s))
            .unwrap_or(Shell::Sh)]
    };
    for shell in shells {
        let agreed = recorded.contains(&shell);
        path_block_one(home, shell, agreed, v);
    }
    registry_item(cx, v);
}

/// The user PATH in the registry, when `install` recorded that it put an entry there.
fn registry_item(cx: &Cx, v: &mut Vec<Item>) {
    let Some(rec) = cigi_home::recorded_registry(&cx.state) else {
        return;
    };
    let what = cigi_home::path::REGISTRY_PATH;
    match cigi_home::registry_path_has(&rec.entry) {
        Ok(true) => v.push(ok(
            "PATH registry",
            format!("{what} has {}", rec.entry.display()),
            what,
        )),
        Ok(false) => v.push(
            act(
                "PATH registry",
                format!(
                    "the PATH entry you agreed to is gone from {what}: {}",
                    rec.entry.display()
                ),
                what,
            )
            .fix("ciginstaller repair")
            .code("E1845"),
        ),
        Err(e) => v.push(note("PATH registry", e.to_string(), what)),
    }
}

fn path_block_one(home: &cigi_home::Home, shell: Shell, agreed: bool, v: &mut Vec<Item>) {
    let (profile, _) = cigi_home::path_block(shell, home);
    match block_state(shell, home) {
        Ok(PathChange::AlreadyPresent) => v.push(ok(
            "PATH block",
            format!("the PATH block is in {}", profile.display()),
            format!("{} for the marked block", profile.display()),
        )),
        Ok(PathChange::NotPresent) if agreed => v.push(
            act(
                "PATH block",
                format!("the PATH block you agreed to is gone from {}", profile.display()),
                format!("{} for the marked block", profile.display()),
            )
            .fix("ciginstaller repair")
            .code("E1845"),
        ),
        Ok(PathChange::NotPresent) => v.push(note(
            "PATH block",
            format!("there is no PATH block in {}", profile.display()),
            "the profile; this machine was installed with --no-path, or before the installer recorded the choice, so I do not add it",
        )),
        Ok(PathChange::Edited) => v.push(
            act(
                "PATH block",
                format!("the block in {} was edited, so it is left alone", profile.display()),
                format!("{} against the block the installer writes", profile.display()),
            )
            .fix(format!(
                "edit {} to delete the lines between the cigscript markers, then run: ciginstaller repair",
                profile.display()
            ))
            .code("E1862"),
        ),
        Ok(PathChange::Skipped) => v.push(note(
            "PATH block",
            format!("{} is a link that leads nowhere usable", profile.display()),
            "the profile path",
        )),
        Ok(_) => {}
        Err(e) => v.push(note("PATH block", e.to_string(), "the profile file")),
    }
}

fn host_with_kernel(cx: &Cx) -> Host {
    let mut h = cx.host.clone();
    if cfg!(target_os = "linux") {
        h.kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
    }
    h
}

fn matrix(cx: &Cx, a: &DoctorArgs, v: &mut Vec<Item>) {
    let m: Option<Manifest> = if a.network {
        match cx.fetch_manifest_uncached() {
            Ok(m) => Some(m),
            Err(e) => {
                v.push(note(
                    "matrix",
                    format!("the manifest could not be fetched: {}", e.message),
                    "the channel's manifest, over the network (--network)",
                ));
                cx.peek_manifest()
            }
        }
    } else {
        cx.peek_manifest()
    };
    let Some(m) = m else {
        v.push(
            note(
                "matrix",
                "there is no manifest to check this machine against",
                "the manifest cache in installer/cache/manifests",
            )
            .fix("ciginstaller doctor --network"),
        );
        return;
    };
    let Some((name, cv)) = cx
        .state
        .machine_carton
        .as_deref()
        .and_then(|c| Version::parse(c).ok().map(|v| (c.to_string(), v)))
    else {
        return;
    };
    let mut items: Vec<(String, Version)> = cx
        .state
        .cartons
        .get(&name)
        .map(|r| {
            r.tools
                .iter()
                .filter_map(|(t, v)| Version::parse(v).ok().map(|v| (t.clone(), v)))
                .collect()
        })
        .unwrap_or_default();
    for s in &cx.state.sticks {
        if let Ok(ver) = Version::parse(&s.version) {
            items.retain(|(t, _)| *t != s.tool);
            items.push((s.tool.clone(), ver));
        }
    }
    let origin = if cx.state.sticks.is_empty() {
        Origin::Carton(cv)
    } else {
        Origin::Stick { carton: cv }
    };
    let found = check(&m, &Combination { items, origin }, &host_with_kernel(cx));
    if found.is_empty() {
        v.push(ok(
            "matrix",
            format!("carton {name} checks out against channel {}", m.channel),
            "every installed product against the manifest's requirements, yanks, advisories and the OS minimums",
        ));
    }
    for f in found {
        let sev = if f.severity == Severity::Refuse {
            Sev::Action
        } else {
            Sev::Note
        };
        let mut it = Item::new(
            "matrix",
            sev,
            f.headline.clone(),
            "the marry-up matrix (`ciginstaller check`)",
        )
        .code(f.code);
        if let Some(fix) = f.fixes.first() {
            it = it.fix(cig_compat::fix_command(fix).to_string());
        }
        v.push(it);
    }
}

/// Programs built on a cig that an advisory of the cached manifest covers (#30): a finding each,
/// read from the cache and the PATH, nothing fetched.
fn programs_item(cx: &Cx, v: &mut Vec<Item>) {
    for (f, advs) in super::programs::advisory_hits(cx) {
        let name = f.program.name.clone();
        v.push(
            // a note, as the matrix's own advisory findings are
            Item::new(
                "programs",
                Sev::Note,
                super::programs::hit_line(cx, &f, &advs),
                "the trailer of each program on PATH and in this project, against the manifest's advisories",
            )
            .fix(format!("ciginstaller programs rebuild {name}")),
        );
    }
}

fn freshness(cx: &Cx, v: &mut Vec<Item>) {
    let st = &cx.state;
    let now = now_epoch();
    let channel = cx.channel();
    // the cache is what `update --check --refresh` renews, so it is what the finding is about
    match cigi_source::fetch::read_fetched(&cx.home, &channel) {
        None if st.machine_carton.is_some() => v.push(
            note(
                "manifest",
                format!("there is no cached manifest for channel {channel}"),
                "installer/cache/manifests",
            )
            .fix("ciginstaller update --check --refresh"),
        ),
        None => {}
        Some(meta) => {
            let expired = cigi_source::clock::parse(&meta.expires).is_some_and(|e| e < now);
            let age_days = cigi_source::clock::parse(&meta.fetched_at).map(|t| (now - t) / 86_400);
            let base = format!(
                "the cached manifest of channel {channel} is sequence {}, fetched {}",
                meta.sequence, meta.fetched_at
            );
            if expired {
                v.push(
                    act(
                        "manifest",
                        format!("{base}, and it expired on {}", meta.expires),
                        "the cached manifest's expiry against today's date",
                    )
                    .fix("ciginstaller update --check --refresh")
                    .code("E1823"),
                );
            } else if age_days.is_some_and(|d| d > 7) {
                v.push(
                    note(
                        "manifest",
                        format!("{base}; that is {} days ago", age_days.unwrap_or(0)),
                        "the cached manifest's age",
                    )
                    .fix("ciginstaller update --check --refresh"),
                );
            } else {
                v.push(ok("manifest", base, "the cached manifest's age and expiry"));
            }
        }
    }
    if let Some(mi) = &st.manifest {
        v.push(ok(
            "manifest",
            format!(
                "the manifest last admitted for an install was sequence {} on channel {}, trust {}",
                mi.sequence, mi.channel, mi.trust_level
            ),
            "state.json's manifest entry",
        ));
    }
    // trust of each install
    let mut by_level: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for (t, ver) in installed(&cx.home) {
        if let Ok(r) = read_receipt(&cx.home, &t, &ver) {
            by_level
                .entry(r.trust.level.clone())
                .or_default()
                .push(format!("{t} {ver}"));
        }
    }
    if !by_level.is_empty() {
        let text: Vec<String> = by_level
            .iter()
            .map(|(l, who)| format!("{l}: {}", who.join(", ")))
            .collect();
        v.push(ok(
            "trust",
            format!("trust level of each install: {}", text.join("; ")),
            "the trust entry of every receipt",
        ));
    }
    // installs that came from one place (a mirror, a directory or a bundle): no second source
    // agreed with them. A bundle is also unsigned until #9, which is why it is said by name.
    let mut one_source: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for (t, ver) in installed(&cx.home) {
        if let Ok(r) = read_receipt(&cx.home, &t, &ver) {
            if !r.trust.independent {
                one_source
                    .entry(r.source.origin.clone())
                    .or_default()
                    .push(format!("{t} {ver}"));
            }
        }
    }
    if !one_source.is_empty() {
        let text: Vec<String> = one_source
            .iter()
            .map(|(origin, who)| {
                let what = if let Some(file) = origin.strip_prefix("bundle:") {
                    format!("the bundle {file}, which is unsigned until #9")
                } else {
                    format!("a mirror ({origin})")
                };
                format!("{} from {what}", who.join(", "))
            })
            .collect();
        v.push(note(
            "trust",
            format!(
                "one source served the manifest and the builds, so no second place agreed with it: {}",
                text.join("; ")
            ),
            "the origin and independence entries of every receipt",
        ));
    }
    let last = st
        .history
        .iter()
        .rev()
        .find(|h| h.ok && (h.what.starts_with("update") || h.what.starts_with("install")));
    if let Some(h) = last {
        v.push(ok(
            "history",
            format!("the last successful {} was at {}", h.what, h.at),
            "state.json's history",
        ));
    }
}

fn disk(cx: &Cx, v: &mut Vec<Item>) {
    let home = &cx.home;
    let du = home.object_store().du();
    let n = installed(home).len();
    let keep_days = cx.cfg.get("keep_days").parse::<u32>().unwrap_or(90);
    let plan = gc::plan(cx, keep_days);
    let what = format!(
        "the store holds {} object(s), {}, shared by {n} toolchain(s) as hard links",
        du.objects,
        size_text(du.bytes)
    );
    if plan.bytes() > 0 || !plan.cartons.is_empty() {
        v.push(
            note(
                "disk",
                format!(
                    "{what}; `gc` would free {} object(s), {}",
                    plan.objects.len(),
                    size_text(plan.bytes())
                ),
                "the sizes in humidor/store/objects and what `gc --dry-run` plans",
            )
            .fix("ciginstaller gc"),
        );
    } else {
        v.push(ok(
            "disk",
            format!("{what}; `gc` has nothing to free"),
            "the sizes in humidor/store/objects and what `gc --dry-run` plans",
        ));
    }
}

// ---- the tin's disk use (#13) ----

/// A tin this big is worth a note: `cig tin gc` is how it shrinks.
const TIN_NOTE_BYTES: u64 = 1 << 30;

/// Files and bytes under a folder, never following a link. Bounded, so a runaway tin cannot hold
/// the doctor up: past the bound the count says "at least".
fn tree_size(root: &Path) -> (u64, u64, bool) {
    const MAX_ENTRIES: u64 = 500_000;
    let (mut files, mut bytes, mut seen) = (0u64, 0u64, 0u64);
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            seen += 1;
            if seen > MAX_ENTRIES {
                return (files, bytes, true);
            }
            let Ok(m) = std::fs::symlink_metadata(e.path()) else {
                continue;
            };
            if m.is_dir() {
                stack.push(e.path());
            } else if m.is_file() {
                files += 1;
                bytes += m.len();
            }
        }
    }
    (files, bytes, false)
}

/// `<home>/tin` beside the store: cig's own content-addressed store of what its runs kept. The
/// installer reads it and never writes it; `cig tin gc` is the fix, run by the person.
fn tin_disk(cx: &Cx, v: &mut Vec<Item>) {
    let tin = cx.home.root().join("tin");
    if !tin.is_dir() {
        v.push(ok(
            "tin",
            "there is no tin yet",
            format!("{} does not exist", tin.display()),
        ));
        return;
    }
    let (files, bytes, cut) = tree_size(&tin);
    let store = cx.home.object_store().du();
    let at_least = if cut { "at least " } else { "" };
    let found = format!(
        "the tin holds {at_least}{} in {at_least}{files} file(s), beside the store's {}",
        size_text(bytes),
        size_text(store.bytes)
    );
    let because = format!("the sizes of the files under {}", tin.display());
    if bytes == 0 {
        v.push(ok("tin", found, because));
    } else if bytes >= TIN_NOTE_BYTES {
        v.push(note("tin", found, because).fix("cig tin gc"));
    } else {
        v.push(ok("tin", found, because).fix("cig tin gc"));
    }
}

// ---- the guard's kernel features (#13) ----

/// The oldest kernel the guard works on: cig's contained box needs 5.12 (cig-orchestrator
/// docs/commands.md, "a kernel before 5.12"), and the BPF LSM hooks need 5.7, so 5.12.
const GUARD_MIN_KERNEL: [u64; 2] = [5, 12];

/// Where doctor reads a kernel file from: the host's own path, or the same path under
/// `CIGI_DOCTOR_PROBE_ROOT` when a test sets it (see the module's header). Read-only either way.
fn probe_path(abs: &str) -> PathBuf {
    match std::env::var_os("CIGI_DOCTOR_PROBE_ROOT").filter(|r| !r.is_empty()) {
        Some(root) => PathBuf::from(root).join(abs.trim_start_matches('/')),
        None => PathBuf::from(abs),
    }
}

fn probe(abs: &str) -> Option<String> {
    std::fs::read_to_string(probe_path(abs))
        .ok()
        .map(|s| s.trim().to_string())
}

fn leading_numbers(text: &str) -> Vec<u64> {
    text.split(|c: char| !c.is_ascii_digit())
        .take_while(|p| !p.is_empty())
        .filter_map(|p| p.parse().ok())
        .collect()
}

/// `numbers` (a kernel's `5.15.0-91`, as `[5, 15, 0, 91]`) is at or above `min` (`[5, 12]`).
fn kernel_at_least(numbers: &[u64], min: [u64; 2]) -> bool {
    let n = |i: usize| numbers.get(i).copied().unwrap_or(0);
    (n(0), n(1)) >= (min[0], min[1])
}

/// A command that brings the kernel up to date, for the package manager this machine has.
fn kernel_update_command() -> Option<&'static str> {
    [
        (
            "/usr/bin/apt-get",
            "sudo apt-get update && sudo apt-get full-upgrade",
        ),
        ("/usr/bin/dnf", "sudo dnf upgrade --refresh"),
        ("/usr/bin/pacman", "sudo pacman -Syu"),
    ]
    .into_iter()
    .find(|(p, _)| Path::new(p).exists())
    .map(|(_, c)| c)
}

/// Read-only probes of the three things the guard's higher levels stand on: the kernel's version,
/// user namespaces, and BPF among the active Linux security modules. Each finding that is not
/// "ok" ends with the one command that fixes it.
fn guard_kernel_features(v: &mut Vec<Item>) {
    if !cfg!(target_os = "linux") && std::env::var_os("CIGI_DOCTOR_PROBE_ROOT").is_none() {
        v.push(note(
            "guard kernel",
            "the guard's kernel features are Linux's, and this is not Linux",
            "the operating system this build runs on",
        ));
        return;
    }
    // a note, never an action: most machines have no BPF LSM, and the guard says so itself when a level needs it
    let sev = Sev::Note;
    let item = |section: &'static str, found: String, because: String, fix: Option<String>| {
        let mut i = Item::new(section, sev, found, because);
        if let Some(f) = fix {
            i = i.fix(f);
        }
        i
    };

    // the kernel against the guard's minimum
    match probe("/proc/sys/kernel/osrelease") {
        None => v.push(note(
            "kernel",
            "I cannot read the kernel's version",
            "/proc/sys/kernel/osrelease",
        )),
        Some(rel) => {
            let min = GUARD_MIN_KERNEL.map(|n| n.to_string()).join(".");
            if kernel_at_least(&leading_numbers(&rel), GUARD_MIN_KERNEL) {
                v.push(ok(
                    "kernel",
                    format!("kernel {rel} is at or above the guard's minimum, {min}"),
                    "/proc/sys/kernel/osrelease",
                ));
            } else {
                v.push(item(
                    "kernel",
                    format!("kernel {rel} is older than the guard's minimum, {min}"),
                    "/proc/sys/kernel/osrelease".to_string(),
                    kernel_update_command().map(str::to_string),
                ));
            }
        }
    }

    // user namespaces: the box's isolation
    match probe("/proc/sys/user/max_user_namespaces").map(|t| t.parse::<u64>()) {
        Some(Ok(0)) => v.push(item(
            "user namespaces",
            "user namespaces are switched off (max_user_namespaces is 0)".to_string(),
            "/proc/sys/user/max_user_namespaces".to_string(),
            Some("sudo sysctl -w user.max_user_namespaces=15000".to_string()),
        )),
        Some(Ok(n)) => v.push(ok(
            "user namespaces",
            format!("user namespaces are on (up to {n})"),
            "/proc/sys/user/max_user_namespaces",
        )),
        Some(Err(_)) => v.push(note(
            "user namespaces",
            "max_user_namespaces holds something that is not a number",
            "/proc/sys/user/max_user_namespaces",
        )),
        // a kernel built without them has no such file, and no command adds it
        None => v.push(item(
            "user namespaces",
            "this kernel has no user namespaces (no max_user_namespaces file)".to_string(),
            "/proc/sys/user/max_user_namespaces does not exist".to_string(),
            None,
        )),
    }

    // BPF LSM: the guard's enforcement from the kernel
    match probe("/sys/kernel/security/lsm") {
        None => v.push(note(
            "BPF LSM",
            "I cannot read the list of active security modules",
            "/sys/kernel/security/lsm (securityfs is not mounted, or it is not readable)",
        )),
        Some(list) if list.split(',').any(|m| m.trim() == "bpf") => v.push(ok(
            "BPF LSM",
            "bpf is among the active security modules",
            "/sys/kernel/security/lsm",
        )),
        Some(list) => {
            let with = if list.is_empty() {
                "bpf".to_string()
            } else {
                format!("{list},bpf")
            };
            v.push(item(
                "BPF LSM",
                format!("bpf is not among the active security modules ({list}); the change takes effect after a reboot"),
                "/sys/kernel/security/lsm".to_string(),
                Some(format!(
                    "sudo sed -i 's/^GRUB_CMDLINE_LINUX=\"/GRUB_CMDLINE_LINUX=\"lsm={with} /' /etc/default/grub && sudo update-grub"
                )),
            ));
        }
    }
}

fn humidor_item(cx: &Cx, v: &mut Vec<Item>) {
    let bad: Vec<String> = humidor_bad(&cx.home)
        .into_iter()
        .map(|(ver, entry_ok, link_ok)| {
            format!(
                "cig {ver} ({})",
                match (entry_ok, link_ok) {
                    (false, false) => "no entry and no link",
                    (false, true) => "no entry in humidor/index.json",
                    _ => "humidor/<ver>/cig is not the installed binary",
                }
            )
        })
        .collect();
    if bad.is_empty() {
        v.push(ok(
            "humidor",
            "the humidor mirror agrees with the installed cig(s)",
            "humidor/index.json and each humidor/<version>/cig against the toolchain's binary",
        ));
    } else {
        v.push(
            act(
                "humidor",
                format!("the humidor mirror is out of step: {}", bad.join("; ")),
                "humidor/index.json and each humidor/<version>/cig against the toolchain's binary",
            )
            .fix("ciginstaller repair"),
        );
    }
}

/// The most a tool's own doctor may say on one stream before the rest is dropped.
const TOOL_OUTPUT_CAP: usize = 256 * 1024;

/// Variables that make a program load someone else's code: not passed to the tool.
fn scrubbed(name: &str) -> bool {
    name == "LD_PRELOAD" || name == "LD_LIBRARY_PATH" || name.starts_with("DYLD_")
}

/// Read a stream up to the cap and send what was read when it ends; the rest is read and dropped
/// so the tool never blocks on a full pipe.
fn capture(mut s: impl Read + Send + 'static) -> std::sync::mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = (&mut s).take(TOOL_OUTPUT_CAP as u64).read_to_end(&mut b);
        let _ = std::io::copy(&mut s, &mut std::io::sink());
        let _ = tx.send(b);
    });
    rx
}

#[cfg(unix)]
fn kill_group(pid: u32) {
    // the tool is its own process group leader: one signal reaches what it started too
    let _ = std::process::Command::new("kill")
        .args(["-KILL", "--", &format!("-{pid}")])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}
#[cfg(not(unix))]
fn kill_group(_pid: u32) {}

/// What a tool said and how it ended: each stream cut at 256 KiB.
#[derive(Debug, Clone, Default)]
pub(super) struct Ran {
    /// The exit code, when the tool ended by itself and not by a signal.
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// Run an installed tool with `args` and return what it said. The tool is the one at `exe`, run
/// in `cwd` (the home, not the project the user stands in) with the library-loading variables
/// removed; `bin_first` puts a directory (the home's `bin`) ahead of `PATH`, for a tool that
/// looks for its siblings there. On a timeout the whole process group is killed and a reader
/// still blocked on a pipe somebody else holds open is not waited for.
pub(super) fn run_args(
    exe: &Path,
    args: &[&str],
    cwd: &Path,
    secs: u64,
    bin_first: Option<&Path>,
) -> Result<Ran, String> {
    use std::process::{Command, Stdio};
    let mut tries = 0;
    let mut child = loop {
        let mut c = Command::new(exe);
        c.args(args)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, _) in std::env::vars_os() {
            if k.to_str().is_some_and(scrubbed) {
                c.env_remove(&k);
            }
        }
        if let Some(bin) = bin_first {
            let mut dirs = vec![bin.to_path_buf()];
            if let Some(p) = std::env::var_os("PATH") {
                dirs.extend(std::env::split_paths(&p));
            }
            if let Ok(joined) = std::env::join_paths(dirs) {
                c.env("PATH", joined);
            }
        }
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut c, 0);
        match c.spawn() {
            Ok(c) => break c,
            // a file written a moment ago can still be open in a child another thread forked
            Err(e) if e.raw_os_error() == Some(26) && tries < 100 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(e) => return Err(e.to_string()),
        }
    };
    let out = child.stdout.take().map(capture);
    let err = child.stderr.take().map(capture);
    let deadline = Instant::now() + Duration::from_secs(secs);
    let mut code = None;
    let finished = loop {
        match child.try_wait() {
            Ok(Some(st)) => {
                code = st.code();
                break true;
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                kill_group(child.id());
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    // after the kill the pipes close, unless something outside the group holds them: wait a
    // moment for what was said, never for the end of a stream
    let wait = if finished {
        Duration::from_secs(5)
    } else {
        Duration::from_millis(300)
    };
    let mut said = [String::new(), String::new()];
    for (slot, rx) in said.iter_mut().zip([out, err]) {
        if let Some(Ok(b)) = rx.map(|r| r.recv_timeout(wait)) {
            *slot = String::from_utf8_lossy(&b).into_owned();
        }
    }
    let [stdout, stderr] = said;
    if finished {
        Ok(Ran {
            code,
            stdout,
            stderr,
        })
    } else {
        Err(format!("did not finish within {secs} seconds"))
    }
}

/// How one tool's `__doctor --json` came out.
enum SelfCheck {
    /// Its findings, in its own order.
    Findings(Vec<ToolFinding>),
    /// This build has no `__doctor` (an unknown subcommand, or an answer that is not JSON).
    Absent,
    /// It could not be run (E1864).
    Failed(String),
    /// It answered JSON that is not the contract's shape (E1879).
    Malformed(String),
}

/// One finding as the tool wrote it (the contract's shape, section 1.7).
struct ToolFinding {
    check: String,
    sev: Sev,
    found: String,
    because: String,
    fix: Option<String>,
}

/// Read `{"tool", "version", "findings": [...]}`. `None` is "not JSON at all"; `Some(Err)` is JSON
/// that is not the shape.
fn parse_self_check(text: &str) -> Option<Result<Vec<ToolFinding>, String>> {
    let v: serde_json::Value = serde_json::from_str(text.trim()).ok()?;
    let Some(list) = v.get("findings").and_then(|f| f.as_array()) else {
        return Some(Err("it has no `findings` list".to_string()));
    };
    let mut out = Vec::new();
    for (i, f) in list.iter().enumerate() {
        let text = |k: &str| f.get(k).and_then(|x| x.as_str());
        let (Some(check), Some(found)) = (text("check"), text("found")) else {
            return Some(Err(format!("finding {} lacks `check` or `found`", i + 1)));
        };
        let sev = match text("severity") {
            Some("ok") => Sev::Ok,
            Some("action") => Sev::Action,
            Some("note") => Sev::Note,
            other => {
                return Some(Err(format!(
                    "finding {} has the severity {}, not ok, note or action",
                    i + 1,
                    other.map_or("(none)".to_string(), |s| format!("`{s}`"))
                )))
            }
        };
        out.push(ToolFinding {
            check: check.to_string(),
            sev,
            found: found.to_string(),
            because: text("because").unwrap_or("").to_string(),
            fix: text("fix").filter(|s| !s.is_empty()).map(str::to_string),
        });
    }
    Some(Ok(out))
}

/// `(tool, version, its self-check)` for each of the machine's installed products. The check is
/// `<tool> __doctor --json`: never `<tool> doctor`, which for cig and cigo is a stub that comes
/// back here.
fn tool_doctors(cx: &Cx) -> Vec<(String, Version, SelfCheck)> {
    let mut out = Vec::new();
    for tool in cigi_shim::TOOLS {
        let Some(ver) = super::family::installed_version(&cx.state, tool) else {
            continue;
        };
        // the doctor does not run a binary it has just found changed
        if !verify_toolchain(&cx.home, tool, &ver).is_empty() {
            out.push((
                tool.to_string(),
                ver,
                SelfCheck::Failed("not run: its toolchain differs from its receipt".to_string()),
            ));
            continue;
        }
        let Some(exe) = super::tool_exe(&cx.home, tool, &ver) else {
            out.push((
                tool.to_string(),
                ver,
                SelfCheck::Failed("not run: no readable receipt".to_string()),
            ));
            continue;
        };
        let check = match run_args(&exe, &["__doctor", "--json"], &cx.home.root(), 60, None) {
            Err(why) => SelfCheck::Failed(why),
            Ok(ran) => match parse_self_check(&ran.stdout) {
                None => SelfCheck::Absent,
                Some(Ok(f)) => SelfCheck::Findings(f),
                Some(Err(why)) => SelfCheck::Malformed(why),
            },
        };
        out.push((tool.to_string(), ver, check));
    }
    out
}

pub fn run(cx: &mut Cx, a: &DoctorArgs) -> Result<Report, InstallerError> {
    // `--tools` is the tools' own doctors and nothing else; by default the installer's findings
    // come first and the tools' follow
    let items = if a.tools { Vec::new() } else { findings(cx, a) };
    let mut actions = items.iter().filter(|i| i.sev == Sev::Action).count();
    let mut notes = items.iter().filter(|i| i.sev == Sev::Note).count();
    let mut first_fix = items
        .iter()
        .find(|i| i.sev == Sev::Action)
        .and_then(|i| i.fix.clone());
    for i in &items {
        let icon = match i.sev {
            Sev::Ok => Icon::Done,
            Sev::Note => Icon::Note,
            Sev::Action => Icon::Refused,
        };
        let mut r = Report::new(icon, format!("{}: {}", i.section, i.found))
            .detail(format!("because I checked {}", i.because));
        if let Some(f) = &i.fix {
            r = r.next(f.clone());
        }
        if let Some(c) = i.code {
            r = r.code(c);
        }
        cx.ui.say(&r);
    }
    let mut tools_json = Vec::new();
    {
        for (tool, ver, check) in tool_doctors(cx) {
            let name = format!("{tool} {ver}");
            cx.ui
                .say(&Report::new(Icon::Note, format!("{name}: its own checks")));
            let (findings, code, said): (Vec<ToolFinding>, Option<&str>, Option<String>) =
                match check {
                    SelfCheck::Findings(f) => (f, None, None),
                    SelfCheck::Absent => (
                        Vec::new(),
                        None,
                        Some(format!(
                            "this {tool} build has no self-check; it needs a newer build"
                        )),
                    ),
                    SelfCheck::Malformed(why) => (
                        Vec::new(),
                        Some("E1879"),
                        Some(format!(
                            "{name} answered its self-check in a shape I cannot read: {why}"
                        )),
                    ),
                    SelfCheck::Failed(why) => (
                        Vec::new(),
                        Some("E1864"),
                        Some(format!("{name} could not be checked: {why}")),
                    ),
                };
            let mut json_findings = Vec::new();
            for f in &findings {
                let icon = match f.sev {
                    Sev::Ok => Icon::Done,
                    Sev::Note => Icon::Note,
                    Sev::Action => Icon::Refused,
                };
                match f.sev {
                    Sev::Action => {
                        actions += 1;
                        if first_fix.is_none() {
                            first_fix = f.fix.clone();
                        }
                    }
                    Sev::Note => notes += 1,
                    Sev::Ok => {}
                }
                let mut r = Report::new(icon, format!("{tool}: {}: {}", f.check, f.found));
                if !f.because.is_empty() {
                    r = r.detail(format!("because I checked {}", f.because));
                }
                if let Some(x) = &f.fix {
                    r = r.next(x.clone());
                }
                cx.ui.say(&r);
                json_findings.push(json!({
                    "section": f.check,
                    "severity": f.sev.word(),
                    "found": f.found,
                    "because": f.because,
                    "fix": f.fix,
                }));
            }
            if let Some(text) = &said {
                notes += 1;
                let mut r = Report::new(Icon::Note, text.clone());
                if let Some(c) = code {
                    r = r.code(c);
                }
                cx.ui.say(&r);
            }
            tools_json.push(json!({
                "tool": name,
                "ok": code.is_none(),
                "self_check": said.is_none(),
                "findings": json_findings,
                "note": said,
                "code": code,
            }));
        }
    }
    let data = json!({
        "needs_action": actions,
        "notes": notes,
        "findings": items.iter().map(Item::json).collect::<Vec<_>>(),
        "tools": tools_json,
    });
    let report = if actions == 0 {
        Report::done(format!(
            "nothing needs action ({} check(s), {notes} note(s))",
            items.len()
        ))
    } else {
        let r = Report::new(Icon::Refused, format!("{actions} thing(s) need action"));
        match first_fix {
            Some(f) => r.next(f),
            None => r,
        }
    };
    Ok(report.data(data))
}

// Unix only: these tests drive shell-script tools to prove the doctor's probes.
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn script(dir: &Path, body: &str) -> PathBuf {
        let p = dir.join("tool");
        let tmp = dir.join("tool.new");
        std::fs::write(&tmp, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::rename(&tmp, &p).unwrap();
        p
    }

    #[test]
    fn the_tool_runs_in_the_directory_it_is_given() {
        let t = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let exe = script(t.path(), "pwd");
        let out = run_args(&exe, &[], home.path(), 20, None).unwrap().stdout;
        assert_eq!(
            Path::new(out.trim()).canonicalize().unwrap(),
            home.path().canonicalize().unwrap()
        );
    }

    #[test]
    fn each_stream_is_cut_at_the_cap() {
        let t = tempfile::tempdir().unwrap();
        let exe = script(
            t.path(),
            "head -c 2000000 /dev/zero | tr '\\0' x; head -c 2000000 /dev/zero | tr '\\0' y >&2",
        );
        let ran = run_args(&exe, &[], t.path(), 30, None).unwrap();
        assert_eq!(ran.stdout.matches('x').count(), TOOL_OUTPUT_CAP);
        assert_eq!(ran.stderr.matches('y').count(), TOOL_OUTPUT_CAP);
    }

    #[test]
    fn a_helper_holding_the_pipe_does_not_hang_the_doctor() {
        let t = tempfile::tempdir().unwrap();
        // the tool starts a helper that keeps the pipes open, then never finishes itself
        let exe = script(t.path(), "echo started; sleep 60 & sleep 60");
        let began = Instant::now();
        let err = run_args(&exe, &[], t.path(), 1, None).unwrap_err();
        assert!(err.contains("did not finish"), "{err}");
        assert!(
            began.elapsed() < Duration::from_secs(10),
            "waited {:?} for a stream",
            began.elapsed()
        );
    }

    #[test]
    fn library_loading_variables_are_scrubbed() {
        for n in [
            "LD_PRELOAD",
            "LD_LIBRARY_PATH",
            "DYLD_INSERT_LIBRARIES",
            "DYLD_FALLBACK_LIBRARY_PATH",
        ] {
            assert!(scrubbed(n), "{n}");
        }
        for n in ["PATH", "HOME", "LD_DEBUG_NOT", "LANG"] {
            assert!(!scrubbed(n), "{n}");
        }
    }
}

/// #47: a home that was moved or restored, or whose builds are not the files their receipts
/// recorded. The shims refuse such a build (E1903, or E1902 when the index leads out of the home);
/// `ciginstaller repair` re-derives the record. Names the cause when it can.
fn moved_home(cx: &Cx, v: &mut Vec<Item>) {
    let home = &cx.home;
    let now = home.root();
    let canon = |p: &std::path::Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let came_from = cx
        .state
        .recorded_home()
        .map(std::path::PathBuf::from)
        .filter(|old| canon(old) != canon(&now));
    // entries of the index that still lead out of this home (the older absolute form)
    let outside: Vec<std::path::PathBuf> = cigi_home::ShimIndex::load(home)
        .ok()
        .flatten()
        .map(|ix| {
            ix.cartons
                .values()
                .flat_map(|t| t.values())
                .chain(ix.sticks.values())
                .filter(|p| p.is_absolute())
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let drifted: Vec<String> = installed(home)
        .into_iter()
        .filter(|(t, ver)| !cigi_home::identity_drift(home, t, ver).is_empty())
        .map(|(t, ver)| format!("{t} {ver}"))
        .collect();
    if came_from.is_none() && outside.is_empty() && drifted.is_empty() {
        return;
    }
    let old = came_from.map(|p| p.display().to_string()).or_else(|| {
        // an index written for another home names it in every absolute path
        outside.first().and_then(|p| {
            let s = p.display().to_string();
            s.find("/toolchains/").map(|i| s[..i].to_string())
        })
    });
    let (found, code) = match (&old, outside.is_empty()) {
        (Some(o), _) => (
            format!(
                "this home was moved from {o}: the shims refuse its builds until the record is made again"
            ),
            if outside.is_empty() { "E1903" } else { "E1902" },
        ),
        (None, true) => (
            "a build in this home is not the file its receipt recorded (the home was restored, or the file was replaced)".to_string(),
            "E1903",
        ),
        (None, false) => (
            "the shim-index leads outside this home".to_string(),
            "E1902",
        ),
    };
    let because = if drifted.is_empty() {
        "state.json's recorded home and the shim-index against this home".to_string()
    } else {
        format!(
            "device, inode, size and time of each build's file against its receipt: {}",
            drifted.join(", ")
        )
    };
    v.push(
        act("moved home", found, because)
            .fix("ciginstaller repair")
            .code(code),
    );
}

/// #46: the home this ciginstaller sits in against the home it works on. A shim in `<home>/bin`
/// works on that home unless `CIGSCRIPT_HOME` says otherwise; when the two differ the finding names
/// both.
fn home_anchor(cx: &Cx, v: &mut Vec<Item>) {
    let Some(own) = std::env::current_exe()
        .ok()
        .and_then(|e| cigi_shim::home_of_exe(&e))
    else {
        return;
    };
    let canon = |p: &std::path::Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let working = cx.home.root();
    if canon(&own) == canon(&working) {
        return;
    }
    let env = std::env::var_os("CIGSCRIPT_HOME")
        .filter(|e| !e.is_empty())
        .map(std::path::PathBuf::from);
    let who = match &env {
        Some(e) => format!("CIGSCRIPT_HOME is {}", e.display()),
        None => format!("--home is {}", working.display()),
    };
    v.push(
        note(
            "home",
            format!(
                "the shims in {} belong to {}, but this run works on {} ({who}, and it wins)",
                own.join("bin").display(),
                own.display(),
                working.display()
            ),
            "where this ciginstaller sits against the home it was told to use",
        )
        .fix(format!(
            "unset CIGSCRIPT_HOME, or set it to {}",
            own.display()
        )),
    );
}
