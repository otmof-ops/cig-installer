// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! Resolution, SPEC section 8: what runs here? The reference the shim's fast path must agree with.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use cig_compat::{legacy_resolve, LegacyOutcome, Pin, Version};
use cigi_ui::{InstallerError, EXIT_REFUSED};

use crate::cx::{Cx, ManifestMode};
use crate::flows::install::{self, InstallArgs, What};
use crate::flows::{cached_yanked, tool_exe};
use crate::project::{file_label, legacy_label};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Read only: never fetches.
    Which,
    /// The slow path of a shim: may fetch what the pin_fetch policy allows.
    Run,
}

#[derive(Debug, Clone)]
pub enum Why {
    Forced(String),
    Pinned(String),
    /// The machine layer locks `carton` (#17); the store that does.
    Locked(String),
    Legacy(String),
    /// A script's `# cig` header: `# cig >=1.6.2 in s.cig`.
    Header(String),
    Machine,
    Stick,
}

#[derive(Debug, Clone)]
pub struct Resolution {
    pub carton: Option<Version>,
    pub build: Version,
    pub path: PathBuf,
    pub why: Why,
    pub steps: Vec<String>,
    /// The script whose header was read, when one was named and carries a pin.
    pub script: Option<ScriptInfo>,
}

/// A script's header as the resolver read it.
#[derive(Debug, Clone)]
pub struct ScriptInfo {
    pub file: PathBuf,
    /// What follows `# cig`, as written: `>=1.6.2`.
    pub header: String,
}

impl Resolution {
    /// `cigo 1.4.1, from carton 0.1.0 (pinned in .cig-carton)`.
    pub fn text(&self, tool: &str) -> String {
        let from = match &self.carton {
            Some(c) => format!("from carton {c}"),
            None => "a stick".to_string(),
        };
        format!("{tool} {}, {from} ({})", self.build, self.why_text())
    }

    /// The reason in words: `pinned in .cig-carton`, `the machine's carton`.
    pub fn why_text(&self) -> String {
        match &self.why {
            Why::Forced(c) => format!("CIGI_CARTON={c}"),
            Why::Pinned(f) => format!("pinned in {f}"),
            Why::Locked(store) => format!("carton locked by {store}"),
            Why::Legacy(f) => format!("satisfies {f}"),
            Why::Header(h) => format!("satisfies {h}"),
            Why::Machine => "the machine's carton".to_string(),
            Why::Stick => "installed on its own".to_string(),
        }
    }
}

fn parsed(s: &str) -> Option<Version> {
    Version::parse(s).ok()
}

/// Installed cartons that hold `tool`, newest first, with the tool's build.
fn holders(cx: &Cx, tool: &str) -> Vec<(Version, Version)> {
    let mut v: Vec<(Version, Version)> = cx
        .state
        .cartons
        .iter()
        .filter_map(|(c, r)| Some((parsed(c)?, parsed(r.tools.get(tool)?)?)))
        .collect();
    v.sort_by(|a, b| b.0.cmp(&a.0));
    v
}

fn found(
    cx: &Cx,
    tool: &str,
    carton: Option<Version>,
    build: Version,
    why: Why,
    steps: Vec<String>,
) -> Result<Resolution, InstallerError> {
    // `rollback <tool>` put one build of the machine carton's tool back: wherever the chosen carton
    // is the machine carton (a pin, CIGI_CARTON, the machine itself) that build runs, exactly as
    // the shim's index has it
    let mut steps = steps;
    let mut build = build;
    if let (Some(c), Some(mc)) = (&carton, cx.state.machine_carton.as_deref()) {
        if c.to_string() == mc {
            if let Some(rb) = cx
                .state
                .sticks
                .iter()
                .find(|s| s.tool == tool && s.is_rollback())
                .and_then(|s| parsed(&s.version))
            {
                if rb != build {
                    steps.push(format!(
                        "   {tool} was rolled back to {rb}, which runs in place of {build} in the machine carton {mc}"
                    ));
                    build = rb;
                }
            }
        }
    }
    let path = tool_exe(&cx.home, tool, &build).ok_or_else(|| {
        InstallerError::new("E1831", format!("{tool} {build} has no readable receipt"))
            .with_fix("ciginstaller repair")
    })?;
    Ok(Resolution {
        carton,
        build,
        path,
        why,
        steps,
        script: None,
    })
}

pub fn resolve(cx: &mut Cx, tool: &str, mode: Mode) -> Result<Resolution, InstallerError> {
    resolve_for(cx, tool, mode, None)
}

/// What a resolution is told besides the tool.
#[derive(Debug, Clone, Copy, Default)]
struct Opts {
    /// A script's header is in play: it wins over `.cig-version`, as cig reads them.
    header_wins: bool,
    /// Leave out the channel's-latest fallback: the caller has a better one.
    no_latest: bool,
}

/// A script header the resolver can act on.
struct Header {
    file: PathBuf,
    /// As written after `# cig`.
    text: String,
    pin: Pin,
    /// `# cig >=1.6.2 in s.cig`.
    label: String,
}

impl Header {
    fn info(&self) -> ScriptInfo {
        ScriptInfo {
            file: self.file.clone(),
            header: self.text.clone(),
        }
    }
}

/// `resolve`, for a command that runs `script` (when one is named): for `cig`, a header in the
/// script (`# cig >=1.6.2`) must be met by the build that runs. The steps of a resolution that
/// ends in an error are left in `cx.last_steps` for `which --why` to print.
pub fn resolve_for(
    cx: &mut Cx,
    tool: &str,
    mode: Mode,
    script: Option<&Path>,
) -> Result<Resolution, InstallerError> {
    let mut steps = Vec::new();
    let mut header = None;
    let locked = locked_carton(cx);
    if let (true, Some(file), None) = (tool == "cig", script, &locked) {
        let file = cx.cwd.join(file);
        if let Some(found) = cigi_shim::read_script_header(&file) {
            let name = file_label(&file);
            match Pin::parse(&found.text) {
                Ok(pin) => {
                    header = Some(Header {
                        label: format!("# cig {} in {name}", found.text),
                        text: found.text,
                        pin,
                        file,
                    })
                }
                Err(_) => steps.push(format!(
                    "   {name} has `# cig {}` on line {}, which is not a constraint ciginstaller reads; cig will say so",
                    found.text, found.line
                )),
            }
        }
    }
    let result = match &header {
        Some(h) => with_header(cx, mode, h, &mut steps, false).map(|mut r| {
            r.script = Some(h.info());
            r
        }),
        None => resolve_inner(cx, tool, mode, false, Opts::default(), &mut steps),
    };
    if result.is_err() {
        cx.last_steps = steps;
    }
    result
}

/// The newest carton of the cached manifest whose cig meets `pin`, not yanked, not a prerelease.
fn fitting_carton(m: &cig_compat::Manifest, pin: &Pin) -> Option<(Version, Version)> {
    m.cartons
        .values()
        .rev()
        .filter(|c| !c.version.is_prerelease() && !m.is_carton_yanked(&c.version))
        .find_map(|c| {
            let b = c.products.get("cig")?;
            pin.satisfied_by(b).then(|| (c.version.clone(), b.clone()))
        })
}

/// Resolve `cig` for a script whose header pins it. In order: the carton the project or the machine
/// would use, when its cig meets the header; else the newest installed cig that does; else, when
/// `pin_fetch` allows, the newest carton on the channel that has one; else E1809.
fn with_header(
    cx: &mut Cx,
    mode: Mode,
    h: &Header,
    steps: &mut Vec<String>,
    fetched: bool,
) -> Result<Resolution, InstallerError> {
    const TOOL: &str = "cig";
    let pin = &h.pin;
    steps.push(format!("H0. {} asks for cig {pin}", h.label));
    // CIGI_CARTON is the user's own word: it runs what it names, and cig hands the run to a
    // matching build from its humidor if that one does not fit
    if std::env::var("CIGI_CARTON").is_ok_and(|s| !s.is_empty()) {
        let mut r = resolve_inner(cx, TOOL, mode, false, Opts::default(), steps)?;
        let verdict = if pin.satisfied_by(&r.build) {
            format!("H1. cig {} meets it", r.build)
        } else {
            format!(
                "H1. cig {} does not meet it; CIGI_CARTON decides, and cig hands the run to a matching build from its humidor",
                r.build
            )
        };
        r.steps.push(verdict);
        return Ok(r);
    }
    let opts = Opts {
        header_wins: true,
        no_latest: true,
    };
    match resolve_inner(cx, TOOL, mode, false, opts, steps) {
        Ok(mut r) if pin.satisfied_by(&r.build) => {
            r.steps.push(format!(
                "H1. cig {} meets it: no other build is looked for",
                r.build
            ));
            return Ok(r);
        }
        Ok(r) => steps.push(format!(
            "H1. cig {} ({}) does not meet it",
            r.build,
            r.why_text()
        )),
        Err(e) if e.code == "E1840" => {
            steps.push("H1. the project and the machine have no cig to offer".to_string())
        }
        Err(e) => return Err(e),
    }

    // the newest installed cig that meets it: a carton's, or a stick's
    let yanked: BTreeSet<String> = cached_yanked(cx).into_iter().collect();
    let mut have: Vec<(Option<Version>, Version)> = holders(cx, TOOL)
        .into_iter()
        .filter(|(c, _)| !yanked.contains(&c.to_string()))
        .map(|(c, b)| (Some(c), b))
        .chain(
            cx.state
                .sticks
                .iter()
                .filter(|s| s.tool == TOOL)
                .filter_map(|s| parsed(&s.version))
                .map(|b| (None, b)),
        )
        .filter(|(_, b)| pin.satisfied_by(b))
        .collect();
    have.sort_by(|a, b| b.1.cmp(&a.1).then(b.0.cmp(&a.0)));
    for (carton, build) in have {
        let mut picked = steps.clone();
        picked.push(format!(
            "H2. the newest installed cig that meets it is {build}{}",
            carton
                .as_ref()
                .map_or(" (installed on its own)".to_string(), |c| format!(
                    ", in carton {c}"
                ))
        ));
        if let Ok(r) = found(
            cx,
            TOOL,
            carton,
            build,
            Why::Header(h.label.clone()),
            picked,
        ) {
            if pin.satisfied_by(&r.build) {
                return Ok(r);
            }
        }
    }
    steps.push("H2. no installed cig meets it".to_string());

    // a carton from the channel
    let listed = cx.peek_manifest().map(|m| fitting_carton(&m, pin));
    let refuse = |why: String, fix: String| {
        InstallerError::new(
            "E1809",
            format!("{} asks for cig {pin}, and {why}", h.label),
        )
        .with_fix(fix)
        .with_exit(EXIT_REFUSED)
    };
    if let Some(None) = listed {
        steps.push(format!(
            "H3. no carton on the {} channel has a cig that meets it",
            cx.channel()
        ));
        return Err(refuse(
            format!(
                "no carton on the {} channel has a cig that meets it",
                cx.channel()
            ),
            "ciginstaller carton list".into(),
        ));
    }
    let named = listed.flatten();
    let fix = named.as_ref().map_or_else(
        || "ciginstaller carton list".to_string(),
        |(c, _)| format!("ciginstaller install carton --carton {c}"),
    );
    let what = named.as_ref().map_or_else(
        || format!("a carton with a cig that meets {pin}"),
        |(c, b)| format!("carton {c} (cig {b})"),
    );
    if mode == Mode::Which {
        steps.push(format!(
            "H3. {what} would be fetched on a run; `which` does not fetch"
        ));
        return Err(refuse("`which` does not fetch".into(), fix));
    }
    if fetched {
        return Err(InstallerError::new(
            "E1856",
            format!(
                "a carton was fetched for {}, but still no cig meets {pin}",
                h.label
            ),
        )
        .with_fix("ciginstaller carton list"));
    }
    if !cx.may_fetch_pinned(&what)? {
        steps.push(format!(
            "H3. {what} is not fetched: pin_fetch, --offline or no terminal to ask on"
        ));
        return Err(refuse(
            "fetching is off (pin_fetch, --offline, or no terminal to ask on)".into(),
            fix,
        ));
    }
    let (carton, build) = match named {
        Some(n) => n,
        None => {
            let m = cx.manifest(ManifestMode::Fresh)?.clone();
            fitting_carton(&m, pin).ok_or_else(|| {
                refuse(
                    format!(
                        "no carton on the {} channel has a cig that meets it",
                        m.channel
                    ),
                    "ciginstaller carton list".into(),
                )
            })?
        }
    };
    cx.ui.say(&cigi_ui::Report::new(
        cigi_ui::Icon::Running,
        format!(
            "fetching carton {carton} for {} (it needs cig {pin}; carton {carton} has cig {build})",
            file_label(&h.file)
        ),
    ));
    let mut a = InstallArgs::carton(Some(carton.to_string()));
    a.no_path = true;
    a.reason = Some(format!("needed by {}", h.label));
    install::run(cx, &a)?;
    steps.clear();
    with_header(cx, mode, h, steps, true)
}

/// Which installed `(carton, build)` runs `tool` under the legacy pins when one tool (`st`) comes
/// from a stick out of `carton` and every other tool from the base carton (SPEC 8 step 3).
fn legacy_pick(
    cx: &Cx,
    tool: &str,
    st: &str,
    carton: &Version,
    pins: &[(String, Pin)],
) -> Option<(Version, Version)> {
    let mine = |b: &Version| {
        pins.iter()
            .filter(|(t, _)| t == tool)
            .all(|(_, p)| p.satisfied_by(b))
    };
    if tool == st {
        let b = cx
            .state
            .cartons
            .get(&carton.to_string())?
            .tools
            .get(tool)
            .and_then(|v| parsed(v))?;
        return mine(&b).then(|| (carton.clone(), b));
    }
    let machine = cx.state.machine_carton.as_deref().and_then(parsed);
    let fits: Vec<(Version, Version)> = holders(cx, tool)
        .into_iter()
        .filter(|(_, b)| mine(b))
        .collect();
    fits.iter()
        .find(|(c, _)| Some(c) == machine.as_ref())
        .or_else(|| fits.first())
        .cloned()
}

/// The carton pin the machine layer locks, and the store that locks it. A lock holding no value
/// pins nothing.
fn locked_carton(cx: &Cx) -> Option<(Pin, String)> {
    let store = cx.cfg.locked_by("carton")?.to_string();
    let pin = Pin::parse(cx.cfg.get("carton").trim()).ok()?;
    Some((pin, store))
}

/// Where the unlocked `carton` setting is read from, for the trail.
fn from_of(cx: &Cx) -> String {
    match cx.cfg.source("carton") {
        "machine" => cx
            .cfg
            .machine_value("carton")
            .map(|(_, s)| s.to_string())
            .unwrap_or_default(),
        other => other.to_string(),
    }
}

/// Run `tool` from the newest installed carton that meets `pin`, fetching one the `pin_fetch`
/// policy allows when none is installed. `label` says where the pin is written, `why` is the
/// reason `which` gives, `step` and `for_what` word the trail and the fetch notice.
#[allow(clippy::too_many_arguments)]
fn pinned_carton(
    cx: &mut Cx,
    tool: &str,
    mode: Mode,
    fetched: bool,
    opts: Opts,
    steps: &mut Vec<String>,
    base_len: usize,
    pin: Pin,
    label: String,
    why: Why,
    step: &str,
    for_what: &str,
) -> Result<Resolution, InstallerError> {
    steps.push(format!("{step} {label} pins carton {pin}"));
    // A range pin (`0.1`, `0`, `>=0.1.0`) passes over a yanked carton; an exact pin still
    // runs it, with a warning. The shim's index leaves yanked cartons out of its series
    // lines for the same reason, so both agree.
    let exact = matches!(pin, Pin::Exact(_));
    let yanked: BTreeSet<String> = cached_yanked(cx).into_iter().collect();
    let allowed = |c: &Version| exact || !yanked.contains(&c.to_string());
    let candidate = holders(cx, tool)
        .into_iter()
        .find(|(c, _)| pin.satisfied_by(c) && allowed(c));
    if let Some((c, b)) = candidate {
        if yanked.contains(&c.to_string()) {
            let w = format!(
                "carton {c} (pinned in {label}) was yanked; it runs because the pin is exact"
            );
            if mode == Mode::Run {
                cx.ui
                    .say(&cigi_ui::Report::new(cigi_ui::Icon::Note, w).code("E1855"));
            } else {
                steps.push(format!("   warning E1855: {w}"));
            }
        }
        return found(cx, tool, Some(c), b, why, steps.clone());
    }
    let any_installed = cx
        .state
        .cartons
        .keys()
        .filter_map(|k| parsed(k))
        .any(|c| pin.satisfied_by(&c) && allowed(&c));
    if any_installed {
        return Err(InstallerError::new(
            "E1840",
            format!("the carton {pin} pinned in {label} has no {tool}"),
        )
        .with_fix(format!("ciginstaller install stick {tool} --carton {pin}")));
    }
    // not installed: the pin_fetch policy
    let fix = format!("ciginstaller install carton --carton {pin}");
    let unavailable = |why: &str| {
        InstallerError::new(
            "E1842",
            format!("carton {pin} (pinned in {label}) is not installed, and {why}"),
        )
        .with_fix(fix.clone())
        .with_exit(EXIT_REFUSED)
    };
    if mode == Mode::Which {
        return Err(unavailable("`which` does not fetch"));
    }
    if fetched {
        return Err(InstallerError::new(
            "E1856",
            format!("carton {pin} was fetched for {label}, but it still does not hold {tool}"),
        )
        .with_fix("ciginstaller carton list"));
    }
    // The policy comes before any request: `never` and --offline make none, and `ask` asks
    // before anything is fetched. The cache only words the question.
    let named = cx
        .peek_manifest()
        .and_then(|m| m.newest_matching(&pin, None).map(|c| c.version.to_string()))
        .unwrap_or_else(|| pin.to_string());
    if !cx.may_fetch_pinned(&format!("carton {named}"))? {
        return Err(unavailable(
            "fetching is off (pin_fetch, --offline, or no terminal to ask on)",
        ));
    }
    cx.ui.say(&cigi_ui::Report::new(
        cigi_ui::Icon::Running,
        format!("fetching carton {named} for {for_what} (pinned in {label})"),
    ));
    let mut a = InstallArgs::carton(Some(pin.to_string()));
    a.no_path = true;
    a.reason = Some(format!("pinned in {label}"));
    install::run(cx, &a)?;
    steps.truncate(base_len);
    resolve_inner(cx, tool, mode, true, opts, steps)
}

fn resolve_inner(
    cx: &mut Cx,
    tool: &str,
    mode: Mode,
    fetched: bool,
    opts: Opts,
    steps: &mut Vec<String>,
) -> Result<Resolution, InstallerError> {
    let base_len = steps.len();
    // L. a carton the machine layer locks beats everything below, CIGI_CARTON included
    let locked = locked_carton(cx);
    if let Some((pin, store)) = &locked {
        steps.push(format!("L1. carton {pin} is locked by {store}; a project's pin, a script's header and CIGI_CARTON do not change it"));
    }
    // 1. CIGI_CARTON
    if let Some(c) = std::env::var("CIGI_CARTON")
        .ok()
        .filter(|s| !s.is_empty() && locked.is_none())
    {
        steps.push(format!("1. CIGI_CARTON is {c}"));
        let cv = parsed(&c).ok_or_else(|| {
            InstallerError::new("E1816", format!("CIGI_CARTON={c} is not a carton version"))
                .with_exit(EXIT_REFUSED)
        })?;
        let rec = cx.state.cartons.get(&cv.to_string()).ok_or_else(|| {
            InstallerError::new("E1816", format!("carton {c} is not installed here"))
                .with_fix(format!("ciginstaller install carton --carton {c}"))
                .with_exit(EXIT_REFUSED)
        })?;
        let b = rec.tools.get(tool).and_then(|v| parsed(v)).ok_or_else(|| {
            InstallerError::new("E1840", format!("carton {c} has no {tool}"))
                .with_fix(format!("ciginstaller install stick {tool} --carton {c}"))
        })?;
        return found(cx, tool, Some(cv), b, Why::Forced(c), steps.clone());
    }
    steps.push("1. CIGI_CARTON is not set".to_string());

    // 2. the project's carton pin
    let project = if locked.is_some() {
        None
    } else {
        cx.project()?
    };
    if let Some(p) = &project {
        steps.push(format!(
            "   project pins are read from {}",
            p.root.display()
        ));
    }
    let chosen = match &locked {
        Some((pin, store)) => Some((
            pin.clone(),
            format!("{store} (locked)"),
            Why::Locked(store.clone()),
            "L2.",
            "this machine",
        )),
        None => project
            .as_ref()
            .and_then(|p| p.carton_pin.clone())
            .map(|(pin, file)| {
                let label = file_label(&file);
                (pin, label.clone(), Why::Pinned(label), "2.", "this project")
            }),
    };
    if let Some((pin, label, why, step, for_what)) = chosen {
        return pinned_carton(
            cx, tool, mode, fetched, opts, steps, base_len, pin, label, why, step, for_what,
        );
    }
    steps.push("2. no carton pin in this project".to_string());

    // 3. legacy pins
    let mut legacy = project
        .as_ref()
        .map(|p| p.legacy.clone())
        .unwrap_or_default();
    if opts.header_wins {
        // a header in the script wins over the file, as cig reads them
        legacy.retain(|(t, p, f)| {
            let drop = t == "cig" && file_label(f) == ".cig-version";
            if drop {
                steps.push(format!(
                    "   {} is set aside: the script's header wins over it",
                    legacy_label(f, p)
                ));
            }
            !drop
        });
    }
    if !legacy.is_empty() {
        let label = legacy
            .iter()
            .map(|(_, p, f)| legacy_label(f, p))
            .collect::<Vec<_>>()
            .join(", ");
        steps.push(format!("3. legacy pin files: {label}"));
        let fits = |cx: &Cx, c: &Version| -> bool {
            let Some(rec) = cx.state.cartons.get(&c.to_string()) else {
                return false;
            };
            rec.tools.contains_key(tool)
                && legacy.iter().all(|(t, pin, _)| {
                    rec.tools
                        .get(t)
                        .and_then(|v| parsed(v))
                        .is_some_and(|v| pin.satisfied_by(&v))
                })
        };
        let mut cartons: Vec<Version> = cx.state.cartons.keys().filter_map(|k| parsed(k)).collect();
        cartons.sort_by(|a, b| b.cmp(a));
        if let Some(c) = cartons.iter().find(|c| fits(cx, c)).cloned() {
            let b = cx.state.cartons[&c.to_string()]
                .tools
                .get(tool)
                .and_then(|v| parsed(v));
            if let Some(b) = b {
                return found(cx, tool, Some(c), b, Why::Legacy(label), steps.clone());
            }
        }
        // nothing installed fits: one tool from a stick, the rest from a base carton
        let pins: Vec<(String, Pin)> = legacy
            .iter()
            .map(|(t, p, _)| (t.clone(), p.clone()))
            .collect();
        let outcome = cx
            .peek_manifest()
            .map(|m| legacy_resolve(&m, &cartons, &pins));
        let have: Vec<String> = pins
            .iter()
            .map(|(t, p)| {
                let built: Vec<String> = holders(cx, t)
                    .iter()
                    .map(|(c, b)| format!("{b} in carton {c}"))
                    .collect();
                format!(
                    "{t} {p} is not met by what is installed ({})",
                    built.join(", ")
                )
            })
            .collect();
        let refusal = |hint: String, fix: String| {
            InstallerError::new(
                "E1801",
                format!("{label} is not met by any installed carton"),
            )
            .with_hint(hint)
            .with_fix(fix)
            .with_exit(EXIT_REFUSED)
        };
        return Err(match outcome {
            Some(LegacyOutcome::Stick { tool: st, carton }) => {
                // already installed (a record of that carton holds the stick's tool)?
                if let Some((c, b)) = legacy_pick(cx, tool, &st, &carton, &pins) {
                    return found(cx, tool, Some(c), b, Why::Legacy(label), steps.clone());
                }
                let fix = format!("ciginstaller install stick {st} --carton {carton}");
                if mode == Mode::Run
                    && !fetched
                    && cx.may_fetch_pinned(&format!("{st} from carton {carton}"))?
                {
                    let mut a = InstallArgs::carton(Some(carton.to_string()));
                    a.what = What::Stick(st.clone());
                    a.no_path = true;
                    a.reason = Some("legacy pin".into());
                    install::run(cx, &a)?;
                    // one look at what was just installed, never another fetch
                    if let Some((c, b)) = legacy_pick(cx, tool, &st, &carton, &pins) {
                        return found(cx, tool, Some(c), b, Why::Legacy(label), steps.clone());
                    }
                    return Err(InstallerError::new(
                        "E1856",
                        format!("{st} was fetched from carton {carton} for {label}, but {tool} is still not met"),
                    )
                    .with_fix("ciginstaller carton list"));
                }
                refusal(have.join("; "), fix)
            }
            Some(LegacyOutcome::None { why }) => refusal(
                [have, why].concat().join("; "),
                "ciginstaller carton list".into(),
            ),
            _ => refusal(have.join("; "), "ciginstaller carton list".into()),
        });
    }
    steps.push("3. no legacy pin files".to_string());

    // 3b. the `carton` setting, from the person's file, the environment or the machine layer's
    // unlocked value: a pin for the whole machine, below every project's own
    if let Ok(pin) = Pin::parse(cx.cfg.get("carton").trim()) {
        let label = format!("the carton setting ({})", from_of(cx));
        return pinned_carton(
            cx,
            tool,
            mode,
            fetched,
            opts,
            steps,
            base_len,
            pin,
            label.clone(),
            Why::Pinned(label),
            "3b.",
            "this machine",
        );
    }

    // 4. the machine carton (rolled-back tools were handled above)
    if let Some(mc) = cx.state.machine_carton.clone().and_then(|c| parsed(&c)) {
        let b = cx
            .state
            .cartons
            .get(&mc.to_string())
            .and_then(|r| r.tools.get(tool))
            .and_then(|v| parsed(v));
        if let Some(b) = b {
            steps.push(format!("4. the machine carton is {mc}"));
            return found(cx, tool, Some(mc), b, Why::Machine, steps.clone());
        }
        steps.push(format!("4. the machine carton {mc} has no {tool}"));
    } else {
        steps.push("4. no machine carton".to_string());
    }
    // 5. a stick
    if let Some(s) = cx.state.sticks.iter().find(|s| s.tool == tool) {
        if let Some(b) = parsed(&s.version) {
            steps.push(format!("5. a stick: {tool} {b}"));
            return found(cx, tool, None, b, Why::Stick, steps.clone());
        }
    }
    // 6. the channel's latest: only a machine with nothing on it falls this far. One that has a
    // carton but not this tool is told how to add the tool, not handed a second carton.
    if cx.state.machine_carton.is_some() || opts.no_latest {
        steps.push(format!("6. no {tool} anywhere on this machine"));
        return Err(
            InstallerError::new("E1840", format!("nothing is installed for `{tool}`"))
                .with_fix("ciginstaller install carton"),
        );
    }
    let latest = cx
        .peek_manifest()
        .and_then(|m| m.latest(false).map(|c| c.version.to_string()));
    let what = latest.as_ref().map_or_else(
        || "the channel's latest carton".to_string(),
        |c| format!("carton {c}, the channel's latest"),
    );
    steps.push(format!(
        "6. nothing is installed or pinned here, so the channel's latest: {what}"
    ));
    let unavailable = |why: &str| {
        InstallerError::new(
            "E1819",
            format!("nothing is installed for `{tool}`, and {what} is not fetched: {why}"),
        )
        .with_fix("ciginstaller install carton")
        .with_exit(EXIT_REFUSED)
    };
    if mode == Mode::Which {
        return Err(unavailable("`which` does not fetch"));
    }
    if fetched {
        return Err(InstallerError::new(
            "E1856",
            format!("{what} was fetched, but it still does not hold {tool}"),
        )
        .with_fix("ciginstaller carton list"));
    }
    if !cx.may_fetch_pinned(&what)? {
        return Err(unavailable(
            "fetching is off (pin_fetch, --offline, or no terminal to ask on)",
        ));
    }
    cx.ui.say(&cigi_ui::Report::new(
        cigi_ui::Icon::Running,
        format!("fetching {what} (nothing is installed here)"),
    ));
    let mut a = InstallArgs::carton(None);
    a.no_path = true;
    a.reason = Some("the channel's latest".into());
    install::run(cx, &a)?;
    steps.truncate(base_len);
    resolve_inner(cx, tool, mode, true, opts, steps)
}
