// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `install [carton [V] | pack <name> | pack --pick a,b | stick <tool>]`, and the questions of #5
//! and #32 when there is a terminal: what are you after, the security preset, crash-report consent.

use std::collections::BTreeMap;

use cig_compat::Manifest;
use cigi_source::{MirrorSource, TrustLevel};
use cigi_ui::{Choice, InstallerError, Mode, Question, Report, EXIT_REFUSED, EXIT_USAGE};
use serde_json::{json, Value};

use super::Outcome;
use crate::cli::{InstallCmd, InstallWhat};
use crate::cx::{Cx, ManifestMode};
use crate::errors::ie;
use crate::flows::bundle;
use crate::flows::install::{
    self, first_run_of, first_run_started, first_run_view, FirstRun, InstallArgs, What, ONCE,
};

/// Said wherever a preset is asked for or shown: choosing one protects nothing yet.
pub const PRESET_NOTE: &str = "recorded only: nothing enforces the preset until cig-orchestrator#66 lands, so choosing paranoid does not turn protection on yet";

/// Can a question reach a person? The same test the prompt API applies before it asks.
fn can_ask(cx: &Cx) -> bool {
    cx.ui.mode != Mode::Json && cx.ui.tty_in && cx.ui.tty_err
}

fn needs_a_target() -> InstallerError {
    InstallerError::new("E1871", "say what to install: a carton, a pack or a stick")
        .with_hint("with no terminal there is nobody to ask")
        .with_fix("ciginstaller install carton")
        .with_exit(EXIT_USAGE)
}

fn usage(msg: impl Into<String>, fix: &str) -> InstallerError {
    InstallerError::new("E1870", msg)
        .with_fix(fix)
        .with_exit(EXIT_USAGE)
}

/// A one-keypress-per-line menu of `names` (keys 1 to 9), through the prompt API.
fn menu(cx: &Cx, text: &str, names: &[String], lines: &[String], default: usize) -> usize {
    let mut q = Question::yes_no(text, true);
    q.choices = names
        .iter()
        .take(9)
        .enumerate()
        .map(|(i, n)| Choice::new(char::from(b'1' + i as u8), n))
        .collect();
    q.default = default.min(q.choices.len().saturating_sub(1));
    for l in lines {
        q = q.detail(l);
    }
    cx.ui.ask(&q).index
}

/// The products of a carton a person can install: everything but the installer itself. A carton
/// with none is a manifest fault, said as a usage error with a way out, never indexed into.
fn installable_tools(c: &cig_compat::Carton) -> Result<Vec<String>, InstallerError> {
    let tools: Vec<String> = c
        .products
        .keys()
        .filter(|t| t.as_str() != "ciginstaller")
        .cloned()
        .collect();
    if tools.is_empty() {
        return Err(usage(
            format!("carton {} holds no product to install", c.version),
            "ciginstaller install carton --carton <another version>  # see `ciginstaller carton list`",
        ));
    }
    Ok(tools)
}

/// #5's first question. Asked only at a terminal; the caller has already refused the other case.
fn ask_what(cx: &mut Cx) -> Result<What, InstallerError> {
    let m: Manifest = cx.manifest(ManifestMode::Fresh)?.clone();
    let latest = m.latest(false).ok_or_else(|| {
        InstallerError::new(
            "E1811",
            format!("the {} channel has no carton to install", m.channel),
        )
        .with_fix("ciginstaller carton list")
        .with_exit(EXIT_REFUSED)
    })?;
    let lines = vec![
        "[1] a stick    one product, e.g. just cig".to_string(),
        "[2] a pack     a few products that go together: dev, builder, security, or pick your own"
            .to_string(),
        "[3] a carton   the whole family (recommended)".to_string(),
        format!(
            "everything comes out of carton {}, so it all matches.",
            latest.version
        ),
    ];
    let kinds: Vec<String> = ["stick", "pack", "carton"].map(String::from).to_vec();
    let tools = installable_tools(latest)?;
    match menu(cx, "what are you after?", &kinds, &lines, 2) {
        0 => {
            let i = menu(cx, "which product?", &tools, &[], 0);
            Ok(What::Stick(
                tools[i.min(tools.len().saturating_sub(1))].clone(),
            ))
        }
        1 => {
            let mut names: Vec<String> = latest.packs.keys().cloned().collect();
            names.push("pick your own".to_string());
            let i = menu(cx, "which pack?", &names, &[], 0);
            if i < names.len() - 1 {
                return Ok(What::Pack(names[i].clone()));
            }
            let mut picked = Vec::new();
            for t in &tools {
                let q = Question::yes_no(&format!("include {t}?"), true);
                if cx.ui.ask(&q).is_yes(&q) {
                    picked.push(t.clone());
                }
            }
            if picked.is_empty() {
                return Err(usage(
                    "no product was picked",
                    "ciginstaller install pack --pick cig,cigo",
                ));
            }
            Ok(What::Pick(picked))
        }
        _ => Ok(What::Carton),
    }
}

/// One choice of a question: the value it sets, the key that picks it and its words.
pub struct Opt {
    pub value: &'static str,
    pub key: char,
    pub label: &'static str,
}

/// One install question as data (#22): what a welcome screen needs to ask it itself, and what
/// `install --dry-run --json` lists. The terminal asks from the same table, so the two agree.
pub struct Spec {
    pub id: &'static str,
    /// The question in the family's voice, and in plain words.
    pub voice: &'static str,
    pub plain: &'static str,
    pub summary: &'static str,
    pub note: Option<&'static str>,
    pub choices: &'static [Opt],
    pub default: &'static str,
    /// The setting the answer is written to; `None` for the question that picks the target.
    pub writes: Option<&'static str>,
    /// Asked once on a machine, then recorded.
    pub once: bool,
}

const fn opt(value: &'static str, key: char, label: &'static str) -> Opt {
    Opt { value, key, label }
}

/// The questions an install can ask, in the order it asks them.
pub const SPECS: [Spec; 3] = [
    Spec {
        id: "what",
        voice: "what are you after?",
        plain: "What to install",
        summary: "a stick is one product, a pack a few that go together, a carton the whole family",
        note: None,
        choices: &[
            opt("stick", '1', "a stick"),
            opt("pack", '2', "a pack"),
            opt("carton", '3', "a carton (recommended)"),
        ],
        default: "carton",
        writes: None,
        once: false,
    },
    Spec {
        id: "preset",
        voice: "how careful should the guard be?",
        plain: "Security preset",
        summary: "the security preset; change it any time with `ciginstaller config set preset`",
        note: Some(PRESET_NOTE),
        choices: &[
            opt("chill", 'c', "chill"),
            opt("balanced", 'b', "balanced"),
            opt("strict", 's', "strict"),
            opt("paranoid", 'p', "paranoid"),
        ],
        default: "balanced",
        writes: Some("preset"),
        once: true,
    },
    Spec {
        id: "reports",
        voice: "send a crash report when a tool crashes?",
        plain: "Send crash reports",
        summary: "nothing leaves this machine without you seeing it first",
        note: Some("nothing leaves this machine without your yes; `ciginstaller crash show` prints exactly what would travel"),
        choices: &[
            opt("always", 'a', "always send"),
            opt("never", 'n', "never send"),
            opt("ask", 'e', "ask each time"),
        ],
        default: "ask",
        writes: Some("reports"),
        once: true,
    },
];

fn spec(id: &str) -> Option<&'static Spec> {
    SPECS.iter().find(|s| s.id == id)
}

/// Ask one of the once-only questions at the terminal; the value of the choice taken.
fn ask_spec(cx: &Cx, sp: &Spec) -> String {
    let mut q = Question::yes_no(sp.voice, true).summary(sp.summary);
    if let Some(n) = sp.note {
        q = q.detail(n);
    }
    q.choices = sp
        .choices
        .iter()
        .map(|o| Choice::new(o.key, o.label))
        .collect();
    q.default = sp
        .choices
        .iter()
        .position(|o| o.value == sp.default)
        .unwrap_or(0);
    let i = cx.ui.ask(&q).index.min(sp.choices.len() - 1);
    sp.choices[i].value.to_string()
}

/// The largest answers file read: a handful of short strings is all one holds.
const ANSWERS_MAX: u64 = 64 * 1024;

/// Is `value` one of the choices of question `id`? A usage error says which are.
fn check_answer(id: &str, value: &str) -> Result<(), InstallerError> {
    let sp = spec(id).filter(|s| s.once).ok_or_else(|| {
        let ids: Vec<&str> = ONCE.to_vec();
        usage(
            if id == "what" {
                "`what` is not an answer: the subcommand (carton, pack or stick) says it"
                    .to_string()
            } else {
                format!(
                    "there is no question `{id}`; the answers are {}",
                    ids.join(", ")
                )
            },
            "ciginstaller install --dry-run --json",
        )
    })?;
    if sp.choices.iter().any(|o| o.value == value) {
        return Ok(());
    }
    let ok: Vec<&str> = sp.choices.iter().map(|o| o.value).collect();
    Err(usage(
        format!(
            "`{value}` is not an answer to {id}; the choices are {}",
            ok.join(", ")
        ),
        "ciginstaller install --dry-run --json",
    ))
}

/// Every answer handed in, from the flags and `--answers`, checked before anything changes.
/// A flag and the file that disagree are refused rather than one quietly winning.
fn gather(c: &InstallCmd) -> Result<BTreeMap<String, String>, InstallerError> {
    let mut given: BTreeMap<String, String> = BTreeMap::new();
    if let Some(path) = &c.answers {
        let bad = |why: String| usage(why, "ciginstaller install --dry-run --json");
        let meta = std::fs::metadata(path).map_err(|e| {
            bad(format!(
                "cannot read the answers file {}: {e}",
                path.display()
            ))
        })?;
        if !meta.is_file() || meta.len() > ANSWERS_MAX {
            return Err(bad(format!(
                "{} is not an answers file: a JSON file under {} KiB",
                path.display(),
                ANSWERS_MAX / 1024
            )));
        }
        let text = std::fs::read_to_string(path).map_err(|e| {
            bad(format!(
                "cannot read the answers file {}: {e}",
                path.display()
            ))
        })?;
        let v: Value = serde_json::from_str(&text)
            .map_err(|e| bad(format!("{} is not JSON: {e}", path.display())))?;
        let obj = v.as_object().ok_or_else(|| {
            bad(format!(
                "{} must be one JSON object, like {{\"preset\": \"strict\"}}",
                path.display()
            ))
        })?;
        for (id, val) in obj {
            let val = val
                .as_str()
                .ok_or_else(|| bad(format!("the answer to {id} must be a string")))?;
            check_answer(id, val)?;
            given.insert(id.clone(), val.to_string());
        }
    }
    for (id, flag) in [("preset", &c.preset), ("reports", &c.reports)] {
        let Some(v) = flag else { continue };
        check_answer(id, v)?;
        match given.get(id) {
            Some(f) if f != v => {
                return Err(usage(
                    format!("{id} is given twice and differently: `{f}` in the answers file, `{v}` on the command line"),
                    "ciginstaller install carton --answers answers.json",
                ))
            }
            _ => {
                given.insert(id.to_string(), v.clone());
            }
        }
    }
    Ok(given)
}

/// Where a question stands on this machine, for this run.
enum Stand {
    /// Handed in by a flag or the answers file.
    Given(String),
    /// Answered on an earlier run; not asked again.
    Recorded(String),
    /// Set before the question came up (`config set`, the environment): left as it is.
    AlreadySet(String),
    /// A person at a terminal would be asked.
    Ask,
    /// The installer does not ask on its own: the first install has passed without a record.
    Closed,
}

impl Stand {
    fn word(&self) -> &'static str {
        match self {
            Stand::Given(_) => "given",
            Stand::Recorded(_) => "answered",
            Stand::AlreadySet(_) => "already-set",
            Stand::Ask => "ask",
            Stand::Closed => "closed",
        }
    }
    fn value(&self) -> Option<&str> {
        match self {
            Stand::Given(v) | Stand::Recorded(v) | Stand::AlreadySet(v) => Some(v),
            _ => None,
        }
    }
}

fn stand(cx: &Cx, id: &str, given: &BTreeMap<String, String>) -> Stand {
    if let Some(v) = given.get(id) {
        return Stand::Given(v.clone());
    }
    let (recorded, done) = first_run_of(&cx.state);
    if let Some(v) = recorded.get(id) {
        return Stand::Recorded(v.clone());
    }
    let open = cx.state.machine_carton.is_none() || (first_run_started(&cx.state) && !done);
    if !open {
        return Stand::Closed;
    }
    let set = match id {
        "preset" => !cx.cfg.get("preset").is_empty(),
        _ => cx.cfg.get_file(id).is_some() || cx.cfg.env_has(id),
    };
    if set {
        Stand::AlreadySet(cx.cfg.get(id))
    } else {
        Stand::Ask
    }
}

/// The questions as data: what the install would ask, and where each stands.
fn questions_json(cx: &Cx, c: &InstallCmd, given: &BTreeMap<String, String>) -> Vec<Value> {
    SPECS
        .iter()
        .map(|sp| {
            let st = if sp.id == "what" {
                if c.what.is_some() {
                    Stand::Given(String::new())
                } else {
                    Stand::Ask
                }
            } else {
                stand(cx, sp.id, given)
            };
            json!({
                "id": sp.id,
                "text": sp.voice,
                "plain": sp.plain,
                "summary": sp.summary,
                "note": sp.note,
                "choices": sp.choices.iter().map(|o| json!({
                    "value": o.value, "key": o.key.to_string(), "label": o.label,
                })).collect::<Vec<_>>(),
                "default": sp.default,
                "writes": sp.writes,
                "once": sp.once,
                "status": st.word(),
                "will_ask": matches!(st, Stand::Ask),
                "answer": st.value().filter(|v| !v.is_empty()),
            })
        })
        .collect()
}

/// `install --dry-run` with nothing named: the questions, and nothing asked, written or fetched.
fn list_questions(cx: &Cx, c: &InstallCmd, given: &BTreeMap<String, String>) -> Report {
    let qs = questions_json(cx, c, given);
    let n = qs.iter().filter(|q| q["will_ask"] == json!(true)).count();
    let mut r = Report::done(format!(
        "dry run: an install asks {} question{}",
        qs.len(),
        if qs.len() == 1 { "" } else { "s" }
    ));
    for (sp, q) in SPECS.iter().zip(&qs) {
        let ch: Vec<&str> = sp.choices.iter().map(|o| o.value).collect();
        r = r.detail(format!(
            "{}: {} [{}] default {}, {}",
            sp.id,
            sp.voice,
            ch.join("|"),
            sp.default,
            q["status"].as_str().unwrap_or("")
        ));
    }
    r.detail(format!("{n} would be asked of a person at a terminal"))
        .next("ciginstaller install carton --answers answers.json")
        .data(json!({
            "dry_run": true,
            "questions": qs,
            "first_run": first_run_view(&cx.state),
        }))
}

pub fn run(cx: &mut Cx, c: &InstallCmd) -> Result<Outcome, InstallerError> {
    // every answer handed in is checked here, before the lock, the manifest or any write
    let given = gather(c)?;
    if let (Some(InstallWhat::Carton { version: Some(v) }), Some(f)) = (&c.what, &c.carton) {
        return Err(usage(
            format!("the carton is given twice: `{v}` and `--carton {f}`"),
            "ciginstaller install carton <version>",
        ));
    }
    // `--dry-run --json` naming nothing is the questions as data: no lock, no manifest, no write
    // (without `--json` it stays the usage error that says to name a target)
    if c.dry_run
        && c.what.is_none()
        && cx.ui.mode == Mode::Json
        && !cx.ui.yes
        && !cx.ui.yes_irreversible
    {
        return Ok(Outcome::Report(list_questions(cx, c, &given)));
    }
    let asking = can_ask(cx) && !cx.ui.yes && !cx.ui.yes_irreversible && !c.dry_run;
    if let Some(ch) = &c.channel {
        cx.cfg
            .set_for_run("channel", ch)
            .map_err(|_| no_such_channel(ch))?;
    }
    // held to the end of the install: dropping it removes the unpacked bundle
    let mut _bundle = None;
    let mut bundle_carton: Option<String> = None;
    if let Some(dir) = &c.from {
        let abs = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.clone());
        if abs.is_file() {
            // a bundle: one file, so one source, admitted exactly as a mirror directory is
            cx.lock_and_recover()?;
            let open = bundle::open_bundle(cx, &abs)?;
            let channel = cx.channel();
            if !open.channels.contains(&channel) {
                match open.channels.as_slice() {
                    [only] if c.channel.is_none() => {
                        cx.cfg
                            .set_for_run("channel", only)
                            .map_err(|_| no_such_channel(only))?;
                    }
                    _ => {
                        let has = open.channels.join(", ");
                        return Err(InstallerError::new(
                            "E1837",
                            format!(
                                "{} holds the manifest of channel {has}, not {channel}",
                                abs.display()
                            ),
                        )
                        .with_fix(format!(
                            "ciginstaller install carton --channel {} --from {}",
                            open.channels.first().map_or("stable", String::as_str),
                            abs.display()
                        ))
                        .with_exit(EXIT_REFUSED));
                    }
                }
            }
            bundle_carton = open.carton.clone();
            _bundle = Some(open);
        } else {
            cx.source = Box::new(MirrorSource::parse(&abs.to_string_lossy()).map_err(ie)?);
        }
    }
    if let Some(level) = &c.require {
        let l = TrustLevel::parse(level).ok_or_else(|| {
            usage(
                format!("`{level}` is not a trust level"),
                "ciginstaller install carton --require checksummed",
            )
        })?;
        cx.require_trust(l);
    }
    // a bundle holds one carton's builds: with none named, that is the carton meant
    let mut carton = c.carton.clone().or(bundle_carton);
    let what = match &c.what {
        None if asking => ask_what(cx)?,
        // unattended, `--yes` alone takes the recommended answer: the whole carton
        None if cx.ui.yes || cx.ui.yes_irreversible => What::Carton,
        None => return Err(needs_a_target()),
        Some(InstallWhat::Carton { version }) => {
            if version.is_some() {
                carton = version.clone();
            }
            What::Carton
        }
        Some(InstallWhat::Stick { tool }) => What::Stick(tool.clone()),
        Some(InstallWhat::Pack { name, pick }) => match (name, pick.is_empty()) {
            (Some(n), true) => What::Pack(n.clone()),
            (None, false) => What::Pick(pick.clone()),
            _ => {
                return Err(usage(
                    "`install pack` takes a pack name or --pick a,b, not both and not neither",
                    "ciginstaller install pack dev",
                ))
            }
        },
    };
    // the settings questions come once: answers handed in are taken as given, and a person at a
    // terminal is asked what is still open on a first install; either way the record is kept
    let mut preset = None;
    let mut reports = None;
    let mut fr = FirstRun::default();
    for id in ONCE {
        let Some(sp) = spec(id) else { continue };
        let (value, how) = match stand(cx, id, &given) {
            Stand::Given(v) => (v, "answers"),
            Stand::AlreadySet(v) => (v, "already-set"),
            Stand::Ask if asking => (ask_spec(cx, sp), "asked"),
            _ => continue,
        };
        fr.answers.insert(id.to_string(), value.clone());
        fr.how.insert(id.to_string(), how.to_string());
        if how == "already-set" {
            continue;
        }
        // a value the setting already has is not written again
        let writes = (how == "answers" && id == "preset") || cx.cfg.get(id) != value;
        match id {
            "preset" if writes => preset = Some(value),
            "reports" if writes => reports = Some(value),
            _ => {}
        }
    }
    let args = InstallArgs {
        what,
        carton,
        no_path: c.no_path,
        shell: c.shell.clone(),
        allow_yanked: c.allow_yanked,
        dry_run: c.dry_run,
        preset,
        reports,
        reason: None,
        first_run: Some(fr).filter(|f| !f.is_empty()),
    };
    if c.dry_run {
        // the plan as before, with the questions beside it
        let mut out = install::run(cx, &args)?;
        if let Some(d) = out.data.as_object_mut() {
            d.insert("questions".into(), json!(questions_json(cx, c, &given)));
            d.insert("first_run".into(), first_run_view(&cx.state));
        }
        return Ok(Outcome::Report(out));
    }
    install::run(cx, &args).map(Outcome::Report)
}

/// E1812: a channel name that is not one of the three.
pub fn no_such_channel(name: &str) -> InstallerError {
    InstallerError::new("E1812", format!("there is no channel `{name}`"))
        .with_hint("the channels are stable, beta and nightly")
        .with_fix("ciginstaller channel stable")
        .with_exit(EXIT_REFUSED)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cig_compat::{Carton, CartonVersion, Version};
    use std::collections::BTreeMap;

    fn carton(products: &[(&str, &str)]) -> Carton {
        Carton {
            version: CartonVersion::parse("0.1.0").expect("version"),
            released: String::new(),
            notes: String::new(),
            tested: Vec::new(),
            products: products
                .iter()
                .map(|(t, v)| (t.to_string(), Version::parse(v).expect("version")))
                .collect::<BTreeMap<_, _>>(),
            packs: BTreeMap::new(),
            advisory: None,
        }
    }

    #[test]
    fn a_carton_with_only_the_installer_is_a_usage_error_not_a_panic() {
        let e = installable_tools(&carton(&[("ciginstaller", "0.1.0")])).unwrap_err();
        assert_eq!(e.code, "E1870");
        assert_eq!(e.exit, EXIT_USAGE);
        assert!(e.fix.is_some_and(|f| f.contains("carton list")));
        let ok = installable_tools(&carton(&[("ciginstaller", "0.1.0"), ("cig", "1.2.0")]));
        assert_eq!(ok.expect("one tool"), vec!["cig".to_string()]);
    }
}
