// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! Resolution, SPEC section 8: what runs here? The reference the shim's fast path must agree with.

use std::collections::BTreeSet;
use std::path::PathBuf;

use cig_compat::{legacy_resolve, LegacyOutcome, Pin, Version};
use cigi_ui::{InstallerError, EXIT_REFUSED};

use crate::cx::Cx;
use crate::flows::install::{self, InstallArgs, What};
use crate::flows::{cached_yanked, tool_exe};
use crate::project::file_label;

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
    Legacy(String),
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
}

impl Resolution {
    /// `cigo 1.4.1, from carton 0.1.0 (pinned in .cig-carton)`.
    pub fn text(&self, tool: &str) -> String {
        let from = match &self.carton {
            Some(c) => format!("from carton {c}"),
            None => "a stick".to_string(),
        };
        let why = match &self.why {
            Why::Forced(c) => format!("CIGI_CARTON={c}"),
            Why::Pinned(f) => format!("pinned in {f}"),
            Why::Legacy(f) => format!("satisfies {f}"),
            Why::Machine => "the machine's carton".to_string(),
            Why::Stick => "installed on its own".to_string(),
        };
        format!("{tool} {}, {from} ({why})", self.build)
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
    })
}

pub fn resolve(cx: &mut Cx, tool: &str, mode: Mode) -> Result<Resolution, InstallerError> {
    resolve_inner(cx, tool, mode, false)
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

fn resolve_inner(
    cx: &mut Cx,
    tool: &str,
    mode: Mode,
    fetched: bool,
) -> Result<Resolution, InstallerError> {
    let mut steps = Vec::new();
    // 1. CIGI_CARTON
    if let Some(c) = std::env::var("CIGI_CARTON").ok().filter(|s| !s.is_empty()) {
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
        return found(cx, tool, Some(cv), b, Why::Forced(c), steps);
    }
    steps.push("1. CIGI_CARTON is not set".to_string());

    // 2. the project's carton pin
    let project = cx.project()?;
    if let Some(p) = &project {
        steps.push(format!(
            "   project pins are read from {}",
            p.root.display()
        ));
    }
    if let Some((pin, file)) = project.as_ref().and_then(|p| p.carton_pin.clone()) {
        let label = file_label(&file);
        steps.push(format!("2. {label} pins carton {pin}"));
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
            return found(cx, tool, Some(c), b, Why::Pinned(label), steps);
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
            format!("fetching carton {named} for this project (pinned in {label})"),
        ));
        let mut a = InstallArgs::carton(Some(pin.to_string()));
        a.no_path = true;
        a.reason = Some(format!("pinned in {label}"));
        install::run(cx, &a)?;
        return resolve_inner(cx, tool, mode, true);
    }
    steps.push("2. no carton pin in this project".to_string());

    // 3. legacy pins
    let legacy = project
        .as_ref()
        .map(|p| p.legacy.clone())
        .unwrap_or_default();
    if !legacy.is_empty() {
        let label = legacy
            .iter()
            .map(|(_, p, f)| format!("{} {p}", file_label(f)))
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
                return found(cx, tool, Some(c), b, Why::Legacy(label), steps);
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
                    return found(cx, tool, Some(c), b, Why::Legacy(label), steps);
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
                        return found(cx, tool, Some(c), b, Why::Legacy(label), steps);
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
            return found(cx, tool, Some(mc), b, Why::Machine, steps);
        }
        steps.push(format!("4. the machine carton {mc} has no {tool}"));
    } else {
        steps.push("4. no machine carton".to_string());
    }
    // 5. a stick
    if let Some(s) = cx.state.sticks.iter().find(|s| s.tool == tool) {
        if let Some(b) = parsed(&s.version) {
            steps.push(format!("5. a stick: {tool} {b}"));
            return found(cx, tool, None, b, Why::Stick, steps);
        }
    }
    // 6. nothing
    Err(
        InstallerError::new("E1840", format!("nothing is installed for `{tool}`"))
            .with_fix("ciginstaller install carton"),
    )
}
