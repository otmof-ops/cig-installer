// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! The marry-up engine (#4): one check, many callers. Findings carry the code,
//! the plain fact as the headline, detail lines and complete fixes; the output
//! grammar (icons, catchphrases) is the caller's.

use crate::manifest::{Manifest, Requirement, Yankable};
use crate::versions::{CartonVersion, Pin, Version};
use crate::{AdvisorySeverity, Target};
use std::collections::BTreeMap;

pub const E_COMBINATION: &str = "E1800";
pub const E_NEEDS: &str = "E1801";
pub const E_STICK: &str = "E1802";
pub const E_YANKED: &str = "E1803";
pub const E_ADVISORY: &str = "E1804";
pub const E_PROGRAM: &str = "E1805";
pub const E_JOURNAL: &str = "E1806";
pub const E_OS_MIN: &str = "E1807";
pub const E_PACKAGE: &str = "E1808";
pub const E_NO_TOOL: &str = "E1814";
pub const E_NO_CARTON: &str = "E1811";
pub const E_NO_BUILD: &str = "E1815";

/// Where a combination of versions came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    /// Exactly the builds of this carton.
    Carton(CartonVersion),
    /// This carton's builds with one or more sticks mixed in. `carton` is the
    /// carton the sticks are added to (the machine carton); items that differ
    /// from it are the sticks.
    Stick { carton: CartonVersion },
    /// What an old per-tool pin file resolved to.
    LegacyPins,
    /// Anything else, put together by hand.
    Mixed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Combination {
    pub items: Vec<(String, Version)>,
    pub origin: Origin,
}

/// The machine a combination has to run on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Host {
    pub os: String,
    pub arch: String,
    pub os_version: Option<String>,
    /// `6.17.0-1033-oem`; only the leading numbers are read.
    pub kernel: Option<String>,
}

impl Host {
    pub fn from_target(t: &Target) -> Host {
        Host {
            os: t.os.clone(),
            arch: t.arch.clone(),
            os_version: None,
            kernel: None,
        }
    }

    pub fn target(&self) -> Target {
        Target {
            os: self.os.clone(),
            arch: self.arch.clone(),
        }
    }
}

/// Ordered so that sorting puts the worst first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Refuse,
    Warn,
    Note,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub code: &'static str,
    pub severity: Severity,
    /// The plain fact, no trailing period. A hard E1801 leads with #4's own words, "these two
    /// don't smoke together."
    pub headline: String,
    pub detail: Vec<String>,
    /// One `→` line each: `<what it does>: <complete command>`. [`fix_command`]
    /// returns the command part.
    pub fixes: Vec<String>,
    /// What the finding is about (`cigo`, `carton 0.0.9`, `CIGI-2026-0001 carton 0.0.9`);
    /// callers key "shown once per version" on `(code, subject)`.
    pub subject: String,
}

/// The command at the end of a fix line.
pub fn fix_command(fix: &str) -> &str {
    fix.rsplit_once(": ").map_or(fix, |(_, c)| c)
}

/// The worst severity among findings.
pub fn worst(findings: &[Finding]) -> Option<Severity> {
    findings.iter().map(|f| f.severity).min()
}

fn finding(
    code: &'static str,
    severity: Severity,
    headline: String,
    detail: Vec<String>,
    fixes: Vec<String>,
    subject: String,
) -> Finding {
    Finding {
        code,
        severity,
        headline,
        detail,
        fixes,
        subject,
    }
}

// ------------------------------------------------------------------ wording

fn short(major: u64, minor: Option<u64>, patch: Option<u64>) -> String {
    let mut s = major.to_string();
    if let Some(m) = minor {
        s.push_str(&format!(".{m}"));
        if let Some(p) = patch {
            s.push_str(&format!(".{p}"));
        }
    }
    s
}

/// A range in words: `>=1.8` is "1.8 or newer", `>=1.16.1, <2` is
/// "1.16.1 or newer, below 2".
pub fn describe_range(r: &Requirement) -> String {
    use semver::Op;
    let parts: Vec<String> = r
        .req
        .comparators
        .iter()
        .map(|c| {
            let v = short(c.major, c.minor, c.patch);
            match c.op {
                Op::GreaterEq => format!("{v} or newer"),
                Op::Greater => format!("newer than {v}"),
                Op::Less => format!("below {v}"),
                Op::LessEq => format!("{v} or older"),
                Op::Exact => format!("exactly {v}"),
                Op::Tilde => format!("{v} (patch releases only)"),
                Op::Caret => format!("{v} (compatible releases)"),
                _ => v,
            }
        })
        .collect();
    if parts.is_empty() {
        "any version".to_string()
    } else {
        parts.join(", ")
    }
}

fn carton_cmd(m: &Manifest, c: &CartonVersion) -> String {
    match m.latest(false) {
        Some(l) if l.version == *c => "ciginstaller update".to_string(),
        _ => format!("ciginstaller carton {c}"),
    }
}

fn leading_numbers(s: &str) -> Vec<u64> {
    let mut out = Vec::new();
    for part in s.split('.') {
        let digits: String = part.chars().take_while(char::is_ascii_digit).collect();
        match digits.parse() {
            Ok(n) => out.push(n),
            Err(_) => break,
        }
        if digits.len() != part.len() {
            break;
        }
    }
    out
}

fn numbers_cmp(a: &[u64], b: &[u64]) -> std::cmp::Ordering {
    let n = a.len().max(b.len());
    for i in 0..n {
        let c = a.get(i).unwrap_or(&0).cmp(b.get(i).unwrap_or(&0));
        if c != std::cmp::Ordering::Equal {
            return c;
        }
    }
    std::cmp::Ordering::Equal
}

/// `kernel 3.10` -> `[3, 10]`; anything else is free text and is not evaluated.
fn kernel_minimum(text: &str) -> Option<Vec<u64>> {
    let rest = text.trim().strip_prefix("kernel")?;
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let n = leading_numbers(rest.trim());
    (!n.is_empty()).then_some(n)
}

// ------------------------------------------------------------------ the engine

fn usable_carton(m: &Manifest, c: &CartonVersion) -> bool {
    !c.is_prerelease() && !m.is_carton_yanked(c)
}

fn requires_fixes(
    m: &Manifest,
    tool: &str,
    ver: &Version,
    dep: &str,
    dep_ver: &Version,
) -> Vec<String> {
    let mut fixes = Vec::new();
    let holds = |c: &CartonVersion| -> Option<(Version, Version)> {
        let carton = m.carton(c)?;
        let t = carton.products.get(tool)?;
        let d = carton.products.get(dep)?;
        let ok = m
            .product(tool, t)
            .and_then(|p| p.requires.get(dep))
            .is_none_or(|r| r.matches(d));
        ok.then(|| (t.clone(), d.clone()))
    };
    // Move forward: the newest carton where the pair fits and the dependency changes.
    if let Some((c, _)) = m
        .cartons
        .keys()
        .rev()
        .filter(|c| usable_carton(m, c))
        .filter_map(|c| holds(c).map(|x| (c, x)))
        .find(|(_, (t, d))| t >= ver && d != dep_ver)
    {
        fixes.push(format!(
            "move to carton {c}, where they match: {}",
            carton_cmd(m, c)
        ));
    }
    // Stay behind: the newest carton with an older tool where the pair fits.
    if let Some((_, (t, _))) = m
        .cartons
        .keys()
        .rev()
        .filter(|c| usable_carton(m, c))
        .filter_map(|c| holds(c).map(|x| (c, x)))
        .find(|(_, (t, d))| t < ver && d == dep_ver)
    {
        fixes.push(format!(
            "or keep {tool} {}.{} for this project: ciginstaller pin {tool} {}.{}",
            t.major, t.minor, t.major, t.minor
        ));
    }
    if fixes.is_empty() {
        fixes.push(format!("get a {dep} that fits: ciginstaller update"));
    }
    fixes
}

fn push_requirement_findings(
    m: &Manifest,
    items: &BTreeMap<&str, &Version>,
    out: &mut Vec<Finding>,
) {
    for (tool, ver) in items {
        let Some(p) = m.product(tool, ver) else {
            continue;
        };
        for (map, severity) in [
            (&p.requires, Severity::Refuse),
            (&p.recommends, Severity::Warn),
        ] {
            for (dep, req) in map {
                let Some(dep_ver) = items.get(dep.as_str()) else {
                    continue;
                };
                if req.matches(dep_ver) {
                    continue;
                }
                let verb = if severity == Severity::Refuse {
                    "needs"
                } else {
                    "works best with"
                };
                // #4's voice for a hard requirement: the catchphrase leads the headline
                let lead = if severity == Severity::Refuse {
                    "these two don't smoke together. "
                } else {
                    ""
                };
                out.push(finding(
                    E_NEEDS,
                    severity,
                    format!(
                        "{lead}{tool} {ver} {verb} {dep} {}; you have {dep_ver}",
                        describe_range(req)
                    ),
                    vec![format!("{tool} {ver} declares {dep} {}", req.text)],
                    requires_fixes(m, tool, ver, dep, dep_ver),
                    (*tool).to_string(),
                ));
            }
        }
    }
}

fn push_yank_findings(m: &Manifest, what: Yankable, out: &mut Vec<Finding>) {
    let Some(y) = m.yank_of(what.clone()) else {
        return;
    };
    let mut fixes = Vec::new();
    match (&what, &y.fixed_in) {
        (Yankable::Carton(_), Some(f)) => {
            fixes.push(format!("move to carton {f}: {}", carton_cmd(m, f)));
        }
        (Yankable::Product(t, _), Some(f)) => {
            fixes.push(format!("move to {t} {f}: ciginstaller update"));
        }
        (Yankable::Carton(_), None) => {
            if let Some(l) = m.latest(false) {
                fixes.push(format!(
                    "move to carton {}: {}",
                    l.version,
                    carton_cmd(m, &l.version)
                ));
            }
        }
        (Yankable::Product(..), None) => {}
    }
    if fixes.is_empty() {
        fixes.push("see what is available: ciginstaller carton list".to_string());
    }
    out.push(finding(
        E_YANKED,
        Severity::Warn,
        format!("{what} was yanked: {}", y.reason),
        Vec::new(),
        fixes,
        what.to_string(),
    ));
}

fn push_advisory_findings(m: &Manifest, subject: &str, ver: &Version, out: &mut Vec<Finding>) {
    for a in &m.advisories {
        if !a
            .affects
            .iter()
            .any(|x| x.subject == subject && x.range.matches_core(ver))
        {
            continue;
        }
        let item = format!("{subject} {ver}");
        let mut detail = Vec::new();
        if !a.summary.is_empty() {
            detail.push(a.summary.clone());
        }
        if let Some(u) = &a.url {
            detail.push(u.clone());
        }
        let fix = match (&a.fixed_in, subject) {
            (Some(f), "carton") => format!("move to carton {f}: {}", carton_cmd(m, f)),
            (Some(f), t) => format!("move to {t} {f}: ciginstaller update"),
            (None, _) => "see what is available: ciginstaller carton list".to_string(),
        };
        out.push(finding(
            E_ADVISORY,
            match a.severity {
                AdvisorySeverity::Security | AdvisorySeverity::Bug => Severity::Warn,
                AdvisorySeverity::Note => Severity::Note,
            },
            format!("{item} is affected by {} ({})", a.id, a.severity.as_str()),
            detail,
            vec![fix],
            format!("{} {item}", a.id),
        ));
    }
}

/// Run every row of #4's table that can be evaluated from the manifest alone.
pub fn check(m: &Manifest, c: &Combination, host: &Host) -> Vec<Finding> {
    let mut out = Vec::new();
    let items: BTreeMap<&str, &Version> = c.items.iter().map(|(t, v)| (t.as_str(), v)).collect();

    let base = match &c.origin {
        Origin::Carton(v) | Origin::Stick { carton: v } => Some(v),
        _ => None,
    };
    if let Some(v) = base {
        match m.carton(v) {
            None => out.push(finding(
                E_NO_CARTON,
                Severity::Refuse,
                format!("carton {v} is not on channel {}", m.channel),
                Vec::new(),
                vec!["see what exists: ciginstaller carton list".to_string()],
                format!("carton {v}"),
            )),
            Some(carton) => {
                push_yank_findings(m, Yankable::Carton(v.clone()), &mut out);
                push_advisory_findings(m, "carton", v, &mut out);
                for (tool, ver) in &items {
                    match carton.products.get(*tool) {
                        Some(w) if w == *ver => {}
                        Some(w) => out.push(finding(
                            E_STICK,
                            Severity::Warn,
                            format!(
                                "{tool} {ver} is not the {tool} in carton {v} ({w}); this mix was never tested together"
                            ),
                            vec![format!("carton {v} was tested with {tool} {w}")],
                            vec![format!(
                                "go back to what carton {v} ships: ciginstaller carton {v}"
                            )],
                            (*tool).to_string(),
                        )),
                        None => out.push(finding(
                            E_STICK,
                            Severity::Warn,
                            format!("{tool} {ver} is not part of carton {v}; this mix was never tested together"),
                            Vec::new(),
                            vec![format!(
                                "go back to what carton {v} ships: ciginstaller carton {v}"
                            )],
                            (*tool).to_string(),
                        )),
                    }
                }
            }
        }
    }

    let installing = base.is_some();
    for (tool, ver) in &items {
        if !m.tools.contains_key(*tool) {
            out.push(finding(
                E_NO_TOOL,
                Severity::Warn,
                format!("{tool} is not a product on channel {}", m.channel),
                vec!["its requirements and builds cannot be checked".to_string()],
                vec!["see what is on this channel: ciginstaller carton list".to_string()],
                (*tool).to_string(),
            ));
            continue;
        }
        push_yank_findings(
            m,
            Yankable::Product((*tool).to_string(), (*ver).clone()),
            &mut out,
        );
        push_advisory_findings(m, tool, ver, &mut out);
        if !m
            .builds
            .contains_key(&((*tool).to_string(), (*ver).clone()))
        {
            out.push(finding(
                E_COMBINATION,
                Severity::Note,
                format!("{tool} {ver} is not in the manifest, so what it needs cannot be checked"),
                Vec::new(),
                vec!["look for a listed build: ciginstaller carton list".to_string()],
                (*tool).to_string(),
            ));
            continue;
        }
        if installing && m.artifact(tool, ver, &host.target()).is_none() {
            out.push(finding(
                E_NO_BUILD,
                Severity::Refuse,
                format!("no {tool} {ver} build for {}-{}", host.os, host.arch),
                Vec::new(),
                vec!["look for a carton that has one: ciginstaller carton list".to_string()],
                (*tool).to_string(),
            ));
        }
        if let (Some(p), Some(kernel)) = (m.product(tool, ver), host.kernel.as_deref()) {
            if let Some(min_text) = p.os_min.get(&host.os) {
                if let Some(min) = kernel_minimum(min_text) {
                    if numbers_cmp(&leading_numbers(kernel), &min) == std::cmp::Ordering::Less {
                        out.push(finding(
                            E_OS_MIN,
                            Severity::Refuse,
                            format!("{tool} {ver} needs {min_text} or newer; this machine has kernel {kernel}"),
                            vec![format!("os_min for {} is `{min_text}`", host.os)],
                            vec!["look for an earlier carton: ciginstaller carton list".to_string()],
                            (*tool).to_string(),
                        ));
                    }
                }
            }
        }
    }
    push_requirement_findings(m, &items, &mut out);
    sort(&mut out);
    out
}

fn sort(f: &mut [Finding]) {
    f.sort_by(|a, b| {
        (a.severity, a.code, &a.subject, &a.headline).cmp(&(
            b.severity,
            b.code,
            &b.subject,
            &b.headline,
        ))
    });
}

/// The project row: every tool pin against what is installed and against the
/// other pins, then the resolved combination through [`check`].
pub fn check_pins(
    m: &Manifest,
    pins: &[(String, Pin)],
    installed: &[(String, Version)],
    host: &Host,
) -> Vec<Finding> {
    let mut out = Vec::new();
    let mut chosen: BTreeMap<String, Version> = BTreeMap::new();
    let mut tools: Vec<&str> = pins.iter().map(|(t, _)| t.as_str()).collect();
    tools.sort_unstable();
    tools.dedup();
    for tool in tools {
        let mine: Vec<&Pin> = pins
            .iter()
            .filter(|(t, _)| t == tool)
            .map(|(_, p)| p)
            .collect();
        let fits = |v: &Version| mine.iter().all(|p| p.satisfied_by(v));
        let have: Vec<&Version> = installed
            .iter()
            .filter(|(t, _)| t == tool)
            .map(|(_, v)| v)
            .collect();
        if let Some(v) = have.iter().filter(|v| fits(v)).max() {
            chosen.insert(tool.to_string(), (*v).clone());
            continue;
        }
        let pin_text = mine
            .iter()
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join(" and ");
        let listed = m.builds_of(tool).into_iter().find(|v| fits(v));
        let have_text = if have.is_empty() {
            "none installed".to_string()
        } else {
            have.iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        };
        let (severity, headline, fixes) = match listed {
            Some(v) => {
                let fix = m
                    .cartons
                    .values()
                    .rev()
                    .filter(|c| usable_carton(m, &c.version))
                    .find(|c| c.products.get(tool) == Some(v))
                    .map(|c| {
                        format!(
                            "move to carton {}, which has it: {}",
                            c.version,
                            carton_cmd(m, &c.version)
                        )
                    })
                    .unwrap_or_else(|| "see what exists: ciginstaller carton list".to_string());
                (
                    Severity::Warn,
                    format!("this project pins {tool} {pin_text}; installed: {have_text}"),
                    vec![fix],
                )
            }
            None if mine.len() > 1 => (
                Severity::Refuse,
                format!(
                    "this project's pins disagree: {tool} {pin_text} no version satisfies both"
                ),
                vec!["loosen one of the pins: ciginstaller check".to_string()],
            ),
            None => (
                Severity::Refuse,
                format!("this project pins {tool} {pin_text}; no such build is listed"),
                vec!["see what exists: ciginstaller carton list".to_string()],
            ),
        };
        out.push(finding(
            E_COMBINATION,
            severity,
            headline,
            Vec::new(),
            fixes,
            tool.to_string(),
        ));
        if let Some(v) = listed {
            chosen.insert(tool.to_string(), v.clone());
        }
    }
    // Tools the project does not pin run as installed (the newest installed build).
    for (tool, ver) in installed {
        let slot = chosen.entry(tool.clone()).or_insert_with(|| ver.clone());
        if !pins.iter().any(|(t, _)| t == tool) && *ver > *slot {
            *slot = ver.clone();
        }
    }
    let combo = Combination {
        items: chosen.into_iter().collect(),
        origin: Origin::LegacyPins,
    };
    out.extend(check(m, &combo, host));
    sort(&mut out);
    out
}

/// A compiled program running: the cig kernel it embeds and the journal format
/// it writes, against the installed cig (E1805, E1806).
pub fn check_program(
    m: &Manifest,
    kernel: &Version,
    journal_format: u32,
    installed_cig: &Version,
) -> Vec<Finding> {
    let mut out = Vec::new();
    let subject = format!("cig {installed_cig}");
    if installed_cig.major != kernel.major || installed_cig < kernel {
        let newer = installed_cig < kernel && installed_cig.major == kernel.major;
        out.push(finding(
            E_PROGRAM,
            Severity::Refuse,
            if newer {
                format!("this program embeds cig {kernel}; the installed cig is older ({installed_cig})")
            } else {
                format!("this program embeds cig {kernel}; the installed cig {installed_cig} is a different major version")
            },
            vec!["it would read or write a run record this cig may not understand".to_string()],
            vec![if newer {
                "get a cig that is new enough: ciginstaller update".to_string()
            } else {
                "rebuild the program with the installed cig: cigcompile build".to_string()
            }],
            subject.clone(),
        ));
    }
    out.extend(check_journal(m, journal_format, installed_cig));
    sort(&mut out);
    out
}

/// A run record's journal format against this cig (E1806): `unburn` on a run made
/// by another version, and the compiled-program row. The format a cig reads is
/// the manifest's `embeds.journal_format` for that cig build.
pub fn check_journal(m: &Manifest, run_format: u32, installed_cig: &Version) -> Vec<Finding> {
    let subject = format!("cig {installed_cig}");
    let Some(known) = m
        .product("cig", installed_cig)
        .and_then(|p| p.journal_format())
    else {
        return vec![finding(
            E_JOURNAL,
            Severity::Note,
            format!("the manifest does not say which journal format cig {installed_cig} reads"),
            Vec::new(),
            vec!["see what is installed: ciginstaller list".to_string()],
            subject,
        )];
    };
    match run_format.cmp(&known) {
        std::cmp::Ordering::Greater => vec![finding(
            E_JOURNAL,
            Severity::Refuse,
            format!("journal format {run_format} is newer than the format {known} that cig {installed_cig} reads"),
            vec!["nothing was read or changed".to_string()],
            vec!["get a cig that reads it: ciginstaller update".to_string()],
            subject,
        )],
        std::cmp::Ordering::Less => vec![finding(
            E_JOURNAL,
            Severity::Note,
            format!("journal format {run_format} is older than the format {known} that cig {installed_cig} writes"),
            Vec::new(),
            vec!["see which carton made it: ciginstaller carton list".to_string()],
            subject,
        )],
        std::cmp::Ordering::Equal => Vec::new(),
    }
}

/// A package's requirements (`(tool, range)` pairs such as `("cigcompile", ">=0.2")`)
/// against what is installed (E1808).
pub fn check_package(
    package: &str,
    requirements: &[(String, String)],
    installed: &[(String, Version)],
) -> Vec<Finding> {
    let mut out = Vec::new();
    for (tool, text) in requirements {
        let subject = format!("{package} {tool}");
        let req = match Requirement::parse(text) {
            Ok(r) => r,
            Err(e) => {
                out.push(finding(
                    E_PACKAGE,
                    Severity::Refuse,
                    format!("{package} asks for {tool} {text}, which is not a version range"),
                    vec![e],
                    vec!["ask the package's author to fix it: ciginstaller check".to_string()],
                    subject,
                ));
                continue;
            }
        };
        match installed.iter().find(|(t, _)| t == tool) {
            None => out.push(finding(
                E_PACKAGE,
                Severity::Refuse,
                format!(
                    "{package} needs {tool} {}; none is installed",
                    describe_range(&req)
                ),
                Vec::new(),
                vec!["install it: ciginstaller install carton".to_string()],
                subject,
            )),
            Some((_, v)) if !req.matches(v) => out.push(finding(
                E_PACKAGE,
                Severity::Refuse,
                format!(
                    "{package} needs {tool} {}; you have {v}",
                    describe_range(&req)
                ),
                Vec::new(),
                vec!["get a newer carton: ciginstaller update".to_string()],
                subject,
            )),
            Some(_) => {}
        }
    }
    sort(&mut out);
    out
}

// ------------------------------------------------------------------ legacy pins

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LegacyOutcome {
    /// The newest installed carton whose builds satisfy every pin.
    Carton(CartonVersion),
    /// No installed carton does, but one tool can be taken as a stick from
    /// `carton` (the carton that carries a satisfying build of `tool`).
    Stick {
        tool: String,
        carton: CartonVersion,
    },
    None {
        why: Vec<String>,
    },
}

/// Pick what satisfies old per-tool pins (`.cig-version`, a `# cig >=1.6.2` header):
/// the newest installed carton whose builds satisfy them all, else a stick.
pub fn legacy_resolve(
    m: &Manifest,
    installed: &[CartonVersion],
    pins: &[(String, Pin)],
) -> LegacyOutcome {
    let mut have: Vec<&CartonVersion> = installed
        .iter()
        .filter(|c| m.cartons.contains_key(*c))
        .collect();
    have.sort_by(|a, b| b.cmp(a));
    have.dedup();
    if have.is_empty() {
        return LegacyOutcome::None {
            why: vec!["no installed carton is on this channel".to_string()],
        };
    }
    let failed = |c: &CartonVersion| -> Vec<&(String, Pin)> {
        pins.iter()
            .filter(|(t, p)| !m.build_in(c, t).is_some_and(|v| p.satisfied_by(v)))
            .collect()
    };
    for c in &have {
        if failed(c).is_empty() {
            return LegacyOutcome::Carton((*c).clone());
        }
    }
    // The base is the installed carton that misses the fewest pins (newest on a tie).
    let mut best: Option<(&CartonVersion, Vec<&(String, Pin)>)> = None;
    for c in &have {
        let f = failed(c);
        if best.as_ref().is_none_or(|(_, b)| f.len() < b.len()) {
            best = Some((c, f));
        }
    }
    let Some((base, misses)) = best else {
        return LegacyOutcome::None { why: Vec::new() };
    };
    let mut tools: Vec<&str> = misses.iter().map(|(t, _)| t.as_str()).collect();
    tools.sort_unstable();
    tools.dedup();
    let mut why: Vec<String> = misses
        .iter()
        .map(|(t, p)| match m.build_in(base, t) {
            Some(v) => format!("{t} {p} is not met by carton {base} ({t} {v})"),
            None => format!("{t} {p} is not met by carton {base} (no {t} in it)"),
        })
        .collect();
    if tools.len() != 1 {
        why.push("more than one tool needs a stick; pin a carton instead".to_string());
        return LegacyOutcome::None { why };
    }
    let tool = tools[0];
    let mine: Vec<&Pin> = pins
        .iter()
        .filter(|(t, _)| t == tool)
        .map(|(_, p)| p)
        .collect();
    let carrying = |only_unyanked: bool| {
        m.cartons.values().rev().find(|c| {
            c.products
                .get(tool)
                .is_some_and(|v| mine.iter().all(|p| p.satisfied_by(v)))
                && (!only_unyanked || !m.is_carton_yanked(&c.version))
        })
    };
    match carrying(true).or_else(|| carrying(false)) {
        Some(c) => LegacyOutcome::Stick {
            tool: tool.to_string(),
            carton: c.version.clone(),
        },
        None => {
            why.push(format!(
                "no carton on this channel carries a {tool} that satisfies it"
            ));
            LegacyOutcome::None { why }
        }
    }
}
