// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `install [carton [V] | pack <name> | pack --pick a,b | stick <tool>]`, and the questions of #5
//! and #32 when there is a terminal: what are you after, the security preset, crash-report consent.

use cig_compat::Manifest;
use cigi_source::{MirrorSource, TrustLevel};
use cigi_ui::{Choice, InstallerError, Mode, Question, EXIT_REFUSED, EXIT_USAGE};

use super::Outcome;
use crate::cli::{InstallCmd, InstallWhat};
use crate::cx::{Cx, ManifestMode};
use crate::errors::ie;
use crate::flows::bundle;
use crate::flows::install::{self, InstallArgs, What};

/// Said wherever a preset is asked for or shown: choosing one protects nothing yet.
pub const PRESET_NOTE: &str = "recorded only: nothing enforces the preset until cig-orchestrator#66 lands, so choosing paranoid does not turn protection on yet";

/// The four presets of cig-orchestrator#66, mildest first.
pub const PRESETS: [&str; 4] = ["chill", "balanced", "strict", "paranoid"];

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

/// The security preset question (cig-orchestrator#66's four). `None` leaves the setting alone.
fn ask_preset(cx: &Cx) -> Option<String> {
    let mut q = Question::yes_no("how careful should the guard be?", true)
        .summary("the security preset; change it any time with `ciginstaller config set preset`")
        .detail(PRESET_NOTE);
    q.choices = PRESETS
        .iter()
        .map(|p| Choice::new(p.chars().next().unwrap_or('b'), p))
        .collect();
    q.default = 1;
    let a = cx.ui.ask(&q);
    Some(PRESETS[a.index.min(PRESETS.len() - 1)].to_string())
}

/// #32's consent question: may a crash report be sent. Answers the `reports` setting.
fn ask_reports(cx: &Cx) -> String {
    let mut q = Question::yes_no("send a crash report when a tool crashes?", true)
        .summary("nothing leaves this machine without you seeing it first")
        .detail("nothing leaves this machine without your yes; `ciginstaller crash show` prints exactly what would travel");
    q.choices = vec![
        Choice::new('a', "always send"),
        Choice::new('n', "never send"),
        Choice::new('e', "ask each time"),
    ];
    q.default = 2;
    match cx.ui.ask(&q).index {
        0 => "always",
        1 => "never",
        _ => "ask",
    }
    .to_string()
}

pub fn run(cx: &mut Cx, c: &InstallCmd) -> Result<Outcome, InstallerError> {
    if let Some(p) = &c.preset {
        if !PRESETS.contains(&p.as_str()) {
            return Err(usage(
                format!(
                    "`{p}` is not a preset; the presets are chill, balanced, strict and paranoid"
                ),
                "ciginstaller install carton --preset balanced",
            ));
        }
    }
    if let (Some(InstallWhat::Carton { version: Some(v) }), Some(f)) = (&c.what, &c.carton) {
        return Err(usage(
            format!("the carton is given twice: `{v}` and `--carton {f}`"),
            "ciginstaller install carton <version>",
        ));
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
    // the settings questions come once: on the first install, at a terminal, when not preset
    let first = cx.state.machine_carton.is_none();
    let mut preset = c.preset.clone();
    let mut reports = None;
    if asking && first {
        if preset.is_none() && cx.cfg.get("preset").is_empty() {
            preset = ask_preset(cx);
        }
        if cx.cfg.get("reports") == "ask" {
            let answer = ask_reports(cx);
            if answer != "ask" {
                reports = Some(answer);
            }
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
    };
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
