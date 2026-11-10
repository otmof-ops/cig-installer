// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! The design standard (#31) and the terminal UI (#32) as behaviour: colour by kind, plain
//! under CI, ASCII icons, light detection, `--wide`, the error label, product names.

use cigi_ui::registry;
use cigi_ui::*;

fn tty() -> Env {
    Env {
        tty_out: true,
        tty_err: true,
        tty_in: true,
        ..Env::default()
    }
    .with("LANG", "en_US.UTF-8")
    .with("TERM", "xterm-256color")
    .with("COLORTERM", "truecolor")
    .with("COLORFGBG", "15;0")
}

fn flags() -> GlobalFlags {
    GlobalFlags::default()
}

/// A terminal whose background nothing has said.
fn unknown() -> Env {
    let mut e = tty();
    e.vars.remove("COLORFGBG");
    e
}

fn ui(env: &Env) -> Ui {
    Ui::from_env(&flags(), env)
}

// ---- themes ----------------------------------------------------------------------------

#[test]
fn light_comes_from_colorfgbg_then_the_background_query_then_the_setting() {
    assert_eq!(ui(&tty()).theme, Theme::Dark, "COLORFGBG says dark");
    assert_eq!(
        ui(&unknown()).theme,
        Theme::Neutral,
        "nothing said: neutral"
    );
    assert!(!ui(&unknown()).theme_known && ui(&tty()).theme_known);
    assert_eq!(ui(&tty().with("COLORFGBG", "0;15")).theme, Theme::Light);
    let mut asked = unknown();
    asked.background = Some((255, 255, 255));
    assert_eq!(ui(&asked).theme, Theme::Light, "OSC 11 said white");
    asked.background = Some((0x1e, 0x1e, 0x2e));
    assert_eq!(ui(&asked).theme, Theme::Dark);
    let mut both = tty().with("COLORFGBG", "15;0");
    both.background = Some((255, 255, 255));
    assert_eq!(ui(&both).theme, Theme::Dark, "COLORFGBG is read first");
    let setting = |theme| Ui::from_env(&GlobalFlags { theme, ..flags() }, &both).theme;
    assert_eq!(setting(ThemeChoice::Light), Theme::Light, "theme = light");
    assert_eq!(setting(ThemeChoice::Dark), Theme::Dark, "theme = dark wins");
}

#[test]
fn light_theme_never_paints_ember_as_text() {
    let (u, _) = ui(&tty().with("COLORFGBG", "0;15")).captured();
    let painted = u.render_report(&Report::done("x").next("ciginstaller update"));
    assert!(
        painted.contains("\x1b[38;2;194;65;12m"),
        "the arrow is ember-ink (#c2410c) on light: {painted:?}"
    );
    assert!(!painted.contains("249;115;22"), "never ember on light");
}

// ---- plain ----------------------------------------------------------------------------

#[test]
fn plain_is_automatic_under_ci_a_screen_reader_and_the_voice_setting() {
    let plain = |env: &Env, f: GlobalFlags| Ui::from_env(&f, env).mode == Mode::Plain;
    assert!(!plain(&tty(), flags()));
    assert!(plain(&tty().with("CI", "true"), flags()));
    assert!(plain(&tty().with("CI", "1"), flags()));
    assert!(!plain(&tty().with("CI", "0"), flags()));
    assert!(!plain(&tty().with("CI", "false"), flags()));
    assert!(plain(&tty().with("CIG_SCREEN_READER", "1"), flags()));
    // KDE sets this on every session by default; reading it would make everyone's output plain
    assert!(!plain(&tty().with("QT_ACCESSIBILITY", "1"), flags()));
    assert!(plain(
        &tty(),
        GlobalFlags {
            voice: VoiceChoice::Plain,
            ..flags()
        }
    ));
    let json = Ui::from_env(
        &GlobalFlags {
            json: true,
            voice: VoiceChoice::Plain,
            ..flags()
        },
        &tty().with("CI", "1"),
    );
    assert_eq!(json.mode, Mode::Json, "structured output wins over plain");
}

#[test]
fn voice_setting_parses() {
    assert_eq!(VoiceChoice::parse("plain"), Some(VoiceChoice::Plain));
    assert_eq!(VoiceChoice::parse("voice"), Some(VoiceChoice::Voice));
    assert_eq!(VoiceChoice::parse("loud"), None);
}

// ---- icons ----------------------------------------------------------------------------

#[test]
fn icons_are_the_ascii_set_on_lang_c_and_on_a_bare_console_font() {
    let icons = |u: &Ui| -> Vec<String> {
        [
            Icon::Done,
            Icon::Running,
            Icon::Ask,
            Icon::Refused,
            Icon::Irreversible,
            Icon::Note,
        ]
        .iter()
        .map(|i| i.glyph(u.ascii).to_string())
        .collect()
    };
    let want = ["[ok]", "[..]", "[?]", "[x]", "[!]", "[-]"];
    assert_eq!(icons(&ui(&Env::default().with("LANG", "C"))), want);
    assert_eq!(icons(&ui(&Env::default())), want, "no locale is C");
    let console = tty().with("TERM", "linux");
    assert_eq!(
        icons(&ui(&console)),
        want,
        "the Linux console font lacks them"
    );
    assert_eq!(
        icons(&ui(&tty())),
        ["✓", "●", "?", "✗", "!", "·"],
        "UTF-8 gets the drawn set"
    );
    assert!(ui(&tty().with("CIG_ASCII", "1")).ascii, "forced on");
    assert!(!ui(&console.with("CIG_ASCII", "0")).ascii, "forced off");
}

#[test]
fn a_prompt_and_a_notice_use_the_same_ascii_icons() {
    let (u, cap) = ui(&Env::default().with("LANG", "C")).captured();
    let p = u.render_prompt(&Question::irreversible("remove everything?"));
    assert!(p.starts_with("[!] remove everything?"), "{p}");
    let p = u.render_prompt(&Question::yes_no("update?", true));
    assert!(p.starts_with("[?] update?"), "{p}");
    u.notice("carton 0.1.2 is out");
    assert!(cap.err().starts_with("[-] carton 0.1.2 is out"));
}

// ---- colour by kind ---------------------------------------------------------------------

fn first_escape(text: &str) -> String {
    let start = text.find('\x1b').expect("an escape");
    let end = start + text[start..].find('m').expect("an m") + 1;
    text[start..end].to_string()
}

#[test]
fn a_refusal_is_painted_by_its_kind_not_always_red() {
    let (u, _) = ui(&tty()).captured();
    let on = |code: &'static str| first_escape(&u.render_error(&InstallerError::new(code, "x")));
    let warn = "\x1b[38;2;251;191;36m";
    let info = "\x1b[38;2;209;213;219m";
    let danger = "\x1b[38;2;248;113;113m";
    assert_eq!(on("E1811"), info, "cant-see-any is info");
    assert_eq!(on("E1816"), info, "dont-have-any is info");
    assert_eq!(on("E1801"), warn, "these-are-mine is warn");
    assert_eq!(on("E1821"), warn, "never-heard-of-you is warn");
    assert_eq!(on("E1820"), warn, "depends reads the OS reason");
    assert_eq!(on("E1890"), danger, "a bug is danger");
    assert_eq!(on("E1870"), danger, "a mistake is danger");
    assert_ne!(on("E1811"), on("E1890"));
}

#[test]
fn every_code_has_a_role_and_the_four_kinds_map_as_issue_31_says() {
    for c in registry::all() {
        let role = registry::role_of(&c.code);
        match c.no.as_str() {
            "cant-see-any" | "dont-have-any" => assert_eq!(role, Role::Info, "{}", c.code),
            "these-are-mine" | "never-heard-of-you" => assert_eq!(role, Role::Warn, "{}", c.code),
            _ => {}
        }
        if c.kind() == Some(Kind::Internal) {
            assert_eq!(role, Role::Danger, "{} is a bug", c.code);
        }
    }
}

#[test]
fn a_finding_that_refuses_is_painted_by_its_kind_too() {
    use cig_compat::check::{Finding, Severity};
    let f = Finding {
        code: "E1801",
        severity: Severity::Refuse,
        headline: "cigo needs cig 1.8".into(),
        detail: vec![],
        fixes: vec![],
        subject: "cigo".into(),
    };
    let (u, _) = ui(&tty()).captured();
    assert_eq!(
        first_escape(&u.render_finding(&f)),
        "\x1b[38;2;251;191;36m",
        "these-are-mine is warn"
    );
}

// ---- the error label --------------------------------------------------------------------

#[test]
fn the_error_label_is_code_and_area_and_the_doctor_line_comes_before_the_fix() {
    let e = InstallerError::new("E1811", "there's no carton 1.4 on stable")
        .with_hint("newest on stable is 1.1.3")
        .with_doctor("I think the channel is wrong, because the manifest lists no 1.4")
        .with_fix("ciginstaller carton 1.1.3");
    let (u, _) = ui(&Env::default().with("LANG", "en_US.UTF-8")).captured();
    let t = u.render_error(&e);
    assert!(
        t.starts_with("✗ error[E1811 carton]: can't see any ciggies bro: there's no carton"),
        "{t}"
    );
    let at = |needle: &str| t.find(needle).unwrap_or_else(|| panic!("{needle} in {t}"));
    assert!(at("= hint:") < at("= explain:"));
    assert!(at("= explain:") < at("= doctor:"));
    assert!(at("= doctor:") < at("= fix:"));
    let (p, _) = Ui::from_env(
        &GlobalFlags {
            plain: true,
            ..flags()
        },
        &Env::default(),
    )
    .captured();
    let t = p.render_error(&e);
    assert!(
        t.starts_with("refused: error[E1811 carton]: wrong address: there's no carton"),
        "{t}"
    );
    assert_eq!(
        e.to_string(),
        "error[E1811 carton]: there's no carton 1.4 on stable"
    );
    let bug = InstallerError::new("E1890", "x");
    assert!(p
        .render_error(&bug)
        .starts_with("bug: error[E1890 internal]:"));
}

#[test]
fn every_area_is_one_lowercase_word() {
    for c in registry::all() {
        let a = registry::area(&c.code);
        assert!(
            !a.is_empty() && a.chars().all(|c| c.is_ascii_lowercase()),
            "{}: {a}",
            c.code
        );
        assert_ne!(a, "installer", "{} has no area", c.code);
    }
}

// ---- names, width -----------------------------------------------------------------------

#[test]
fn products_are_named_as_products_in_output() {
    assert_eq!(
        product_names("cigcompile 0.2.0 installed"),
        "CigCompile 0.2.0 installed"
    );
    assert_eq!(
        product_names("cigar 0.1.0, cigo 1.17.0"),
        "CigarScript 0.1.0, cigo 1.17.0"
    );
    assert_eq!(product_names("cigarscript"), "CigarScript");
    assert_eq!(product_names("cigcompile"), "CigCompile");
    assert_eq!(product_names("cigar"), "CigarScript");
    // commands, paths, flags and file names stay as typed
    for keep in [
        "run `cigcompile build`",
        "~/.cigscript/toolchains/cigcompile/0.2.0/bin/cigcompile",
        "cigar check",
        "cigar-version",
        "cig-compile cigar-script",
        "--tool cigcompile=0.2",
        "cigcompile.toml",
        "mycigar",
    ] {
        assert_eq!(product_names(keep), keep);
    }
}

#[test]
fn product_names_reach_headlines_and_tables_but_not_the_next_step() {
    let (u, _) = ui(&Env::default().with("LANG", "en_US.UTF-8")).captured();
    let r = Report::done("carton 0.1.0 installed: cigcompile 0.2.0")
        .detail("cigar 0.1.0 is in the carton")
        .next("ciginstaller install stick cigcompile");
    let t = u.render_report(&r);
    assert!(t.contains("CigCompile 0.2.0"), "{t}");
    assert!(t.contains("CigarScript 0.1.0"), "{t}");
    assert!(
        t.contains("→ ciginstaller install stick cigcompile"),
        "the command is typed as is: {t}"
    );
    let table = Table::new(&[("product", Align::Left)]).row(vec!["cigcompile".into()]);
    assert!(u.render_table(&table).contains("CigCompile"));
    // --json carries ids, never prose names
    let (j, cap) = Ui::from_env(
        &GlobalFlags {
            json: true,
            ..flags()
        },
        &Env::default(),
    )
    .captured();
    j.finish(
        "install",
        &Ok(r.data(serde_json::json!({"tool": "cigcompile"}))),
    );
    assert!(
        cap.out().contains("\"tool\":\"cigcompile\""),
        "{}",
        cap.out()
    );
}

#[test]
fn wide_shows_every_column_in_full() {
    let table = Table::new(&[("a", Align::Left), ("what", Align::Left)])
        .row(vec!["x".into(), "y".repeat(200).as_str().into()]);
    let (narrow, _) = ui(&tty()).captured();
    assert!(narrow.render_table(&table).contains('…'));
    let (wide, _) = Ui::from_env(
        &GlobalFlags {
            wide: true,
            ..flags()
        },
        &tty(),
    )
    .captured();
    let t = wide.render_table(&table);
    assert!(!t.contains('…') && t.contains(&"y".repeat(200)), "{t}");
}

// ---- keypresses -------------------------------------------------------------------------

#[test]
fn one_key_answers_and_enter_takes_the_default() {
    let (u, cap) = ui(&tty()).captured();
    let q = Question::yes_no("go on?", true);
    cap.push_input("n");
    let a = u.ask(&q);
    assert_eq!((a.key, a.how), ('n', How::Typed));
    cap.push_input("");
    assert_eq!(u.ask(&q).how, How::Enter);
    cap.push_input("N");
    assert_eq!(u.ask(&q).key, 'n', "case does not matter");
    cap.push_input("q");
    cap.push_input("y");
    let a = u.ask(&q);
    assert_eq!(a.key, 'y');
    assert!(cap.err().contains("pick one of: y n"), "{}", cap.err());
}
