// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0

//! The behaviour tests of SPEC 14.2: escape codes, safe defaults, line-based progress,
//! ASCII fallback, plain facts, detection, palette and the registry.

use cig_compat::check::{Finding, Severity};
use cigi_ui::registry::{self, number};
use cigi_ui::tokens::{paint, sgr};
use cigi_ui::*;
use std::collections::BTreeSet;

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
}

fn flags() -> GlobalFlags {
    GlobalFlags::default()
}

fn every_screen(ui: &Ui) {
    ui.say(&Report::new(Icon::Running, "fetching"));
    ui.say(&Report::done("done").detail("detail → x").next("next"));
    ui.say(&Report::new(Icon::Refused, "no").code("E1811"));
    ui.error(&InstallerError::new("E1811", "no such carton"));
    ui.error(&InstallerError::new("E1890", "bug"));
    ui.table(
        &Table::new(&[("a", Align::Left), ("b", Align::Left)])
            .row(vec![Cell::changed("x — y"), "…".into()]),
    );
    ui.notice("a notice");
    let mut p = ui.progress("fetching", Some(100));
    p.set_position(60);
    p.finish(Some("fetched"));
    ui.ask(&Question::irreversible("sure?"));
    ui.finding(&Finding {
        code: "E1801",
        severity: Severity::Warn,
        headline: "h".into(),
        detail: vec![],
        fixes: vec!["fix".into()],
        subject: "s".into(),
    });
}

#[test]
fn no_escape_codes_without_a_tty() {
    let cases = [
        (Env::default().with("COLORTERM", "truecolor"), flags()),
        (tty().with("NO_COLOR", "1"), flags()),
        (tty().with("TERM", "dumb"), flags()),
        (
            tty(),
            GlobalFlags {
                no_color: true,
                ..flags()
            },
        ),
        (
            tty(),
            GlobalFlags {
                json: true,
                ..flags()
            },
        ),
    ];
    for (i, (env, f)) in cases.into_iter().enumerate() {
        let (ui, cap) = Ui::from_env(&f, &env).captured();
        every_screen(&ui);
        let all = cap.out() + &cap.err();
        assert!(!all.contains('\x1b'), "case {i} leaked an escape: {all:?}");
        assert!(
            env.tty_err || !all.contains('\r'),
            "case {i} used a carriage return"
        );
        assert!(!all.is_empty() || f.json);
    }
    // and with a tty and colour the painted output does have them
    let (ui, cap) = Ui::from_env(&flags(), &tty()).captured();
    ui.say(&Report::done("x"));
    assert!(cap.out().contains("\x1b[38;2;"));
}

#[test]
fn stdout_and_stderr_are_judged_separately() {
    let env = Env {
        tty_out: false,
        tty_err: true,
        ..tty()
    };
    let ui = Ui::from_env(&flags(), &env);
    assert_eq!(ui.color, ColorMode::None);
    assert_eq!(ui.color_err, ColorMode::True);
}

#[test]
fn safe_default_without_a_tty() {
    let (ui, cap) = Ui::from_env(&flags(), &Env::default()).captured();
    let q = Question::irreversible("remove everything?");
    let a = ui.ask(&q);
    assert_eq!((a.key, a.how, a.refusal), ('n', How::NoTty, true));
    assert_eq!(a.exit_code(), Some(3));
    assert!(cap.err().contains("no terminal to ask on"));
    // a non-refusing default does not force exit 3
    let a = ui.ask(&Question::yes_no("go on?", true));
    assert_eq!((a.key, a.exit_code()), ('y', None));
    // --yes alone does not say yes to an irreversible step; --yes-irreversible does
    let (ui, _) = Ui::from_env(
        &GlobalFlags {
            yes: true,
            ..flags()
        },
        &Env::default(),
    )
    .captured();
    assert_eq!(ui.ask(&q).exit_code(), Some(3));
    assert!(ui
        .ask(&Question::yes_no("go on?", false))
        .is_yes(&Question::yes_no("go on?", false)));
    let (ui, _) = Ui::from_env(
        &GlobalFlags {
            yes: true,
            yes_irreversible: true,
            ..flags()
        },
        &Env::default(),
    )
    .captured();
    let a = ui.ask(&q);
    assert_eq!((a.how, a.key, a.exit_code()), (How::Flag, 'y', None));
    // json never asks either
    let (ui, cap) = Ui::from_env(
        &GlobalFlags {
            json: true,
            ..flags()
        },
        &tty(),
    )
    .captured();
    assert_eq!(ui.ask(&q).exit_code(), Some(3));
    assert_eq!(cap.err(), "");
}

#[test]
fn asking_on_a_terminal() {
    let (ui, cap) = Ui::from_env(&flags(), &tty()).captured();
    let q = Question::yes_no("update?", true).choice(Choice::new('s', "show changes"));
    cap.push_input("");
    assert_eq!(ui.ask(&q).how, How::Enter);
    cap.push_input("N");
    let a = ui.ask(&q);
    assert_eq!((a.key, a.how, a.refusal), ('n', How::Typed, true));
    cap.push_input("x");
    cap.push_input("s");
    assert_eq!(ui.ask(&q).key, 's');
    assert!(cap.err().contains("pick one of: y n s"));
    let a = ui.ask(&q); // end of input
    assert_eq!((a.how, a.key), (How::NoTty, 'y'));
    let irr = Question::irreversible("sure?");
    cap.push_input("");
    let a = ui.ask(&irr);
    assert_eq!((a.key, a.how), ('n', How::Enter));
    assert_eq!(
        a.exit_code(),
        None,
        "a refusal typed at a terminal is not a usage error"
    );
}

#[test]
fn progress_is_line_based_when_piped() {
    let (ui, cap) = Ui::from_env(&flags(), &Env::default()).captured();
    let mut p = ui.progress("fetching carton 0.1.0", Some(1000));
    for step in [10, 240, 1, 260, 300, 189, 0, 1] {
        p.advance(step);
    }
    p.advance(0);
    p.finish(None);
    let err = cap.err();
    assert!(!err.contains('\r') && !err.contains('\x1b'));
    let pcts: Vec<&str> = err.lines().filter_map(|l| l.split("  ").nth(1)).collect();
    assert_eq!(pcts, ["0%", "25%", "50%", "75%", "100%"], "{err}");
    // unknown total: a line at the start and at the end
    let (ui, cap) = Ui::from_env(&flags(), &Env::default()).captured();
    let mut p = ui.progress("scanning", None);
    p.advance(5);
    p.finish(Some("scanned"));
    assert_eq!(cap.err().lines().count(), 2);
    // json and quiet: silent
    for f in [
        GlobalFlags {
            json: true,
            ..flags()
        },
        GlobalFlags {
            quiet: true,
            ..flags()
        },
    ] {
        let (ui, cap) = Ui::from_env(&f, &tty()).captured();
        let mut p = ui.progress("x", Some(10));
        p.advance(10);
        p.finish(Some("x"));
        assert_eq!(cap.err() + &cap.out(), "");
    }
}

#[test]
fn plain_progress_at_a_terminal_is_start_and_end_only() {
    let (ui, cap) = Ui::from_env(
        &GlobalFlags {
            plain: true,
            ..flags()
        },
        &tty(),
    )
    .captured();
    let mut p = ui.progress("fetching", Some(100));
    p.set_position(50);
    p.finish(Some("fetched"));
    let err = cap.err();
    assert_eq!(err.lines().collect::<Vec<_>>().len(), 2, "{err}");
    assert!(!err.contains('\r'));
}

#[test]
fn live_bar_draws_and_collapses_to_a_done_line() {
    let (ui, cap) = Ui::from_env(&flags(), &tty()).captured();
    let mut p = ui.progress("fetching", Some(100));
    p.finish(Some("fetched"));
    let err = cap.err();
    assert!(
        err.starts_with("● fetching") || err.starts_with("\r"),
        "{err:?}"
    );
    assert!(err.ends_with("fetched\n"));
    assert!(err.contains("\x1b[0m"));
}

#[test]
fn icons_fall_back_to_ascii() {
    let (ui, cap) = Ui::from_env(&flags(), &Env::default().with("LANG", "C")).captured();
    assert!(ui.ascii);
    every_screen(&ui);
    let all = cap.out() + &cap.err();
    assert!(all.is_ascii(), "non-ASCII in: {all}");
    assert!(
        all.contains("[ok] done")
            && all.contains("[..] fetching")
            && all.contains("[x] error[E1811")
            && all.contains("[-] a notice"),
        "{all}"
    );
    assert!(ui
        .render_prompt(&Question::irreversible("sure?"))
        .starts_with("? sure?"));
    assert!(all.contains("-> next"));
    let (ui, cap) = Ui::from_env(&flags(), &Env::default().with("LANG", "en_US.UTF-8")).captured();
    ui.say(&Report::done("x").next("y"));
    assert!(cap.out().starts_with("✓ x") && cap.out().contains("→ y"));
    // the bar
    let (ui, _) = Ui::from_env(&flags(), &Env::default().with("LANG", "C")).captured();
    let line = render_bar(
        &ui,
        "f",
        Unit::Bytes,
        5,
        Some(10),
        std::time::Duration::from_secs(1),
    );
    assert!(
        line.is_ascii() && line.contains("[#####") && line.starts_with("[..] f"),
        "{line}"
    );
}

#[test]
fn plain_has_the_same_facts() {
    let facts = [
        "carton 0.1.0 installed",
        "cig 1.20.0",
        "pinned 1",
        "ciginstaller rollback puts 0.1.0 back",
        "E1811",
        "no carton 1.4",
        "newest on stable is 0.1.1",
        "ciginstaller carton 0.1.1",
    ];
    let run = |ui: &Ui| {
        ui.say(
            &Report::done("carton 0.1.0 installed: cig 1.20.0")
                .detail("trust: pinned 1")
                .next("ciginstaller rollback puts 0.1.0 back"),
        );
        ui.say(&Report::new(Icon::Refused, "there's no carton 1.4 on stable").code("E1811"));
        ui.error(
            &InstallerError::new("E1811", "there's no carton 1.4")
                .with_hint("newest on stable is 0.1.1")
                .with_fix("ciginstaller carton 0.1.1"),
        );
    };
    let (voice, vcap) =
        Ui::from_env(&flags(), &Env::default().with("LANG", "en_US.UTF-8")).captured();
    let (plain, pcap) = Ui::from_env(
        &GlobalFlags {
            plain: true,
            ..flags()
        },
        &Env::default().with("LANG", "en_US.UTF-8"),
    )
    .captured();
    run(&voice);
    run(&plain);
    let (v, p) = (vcap.out() + &vcap.err(), pcap.out() + &pcap.err());
    for f in facts {
        assert!(v.contains(f), "voice lacks {f}");
        assert!(p.contains(f), "plain lacks {f}");
    }
    assert!(
        v.contains("can't see any ciggies bro"),
        "voice has the catchphrase"
    );
    assert!(!p.contains("ciggies"), "plain drops it: {p}");
    assert!(
        p.contains("ok: carton")
            && p.contains("refused: ")
            && p.contains("  next: ciginstaller rollback")
    );
}

#[test]
fn the_code_is_right_aligned_in_voice() {
    let (ui, cap) = Ui::from_env(
        &flags(),
        &Env::default()
            .with("LANG", "en_US.UTF-8")
            .with("COLUMNS", "60"),
    )
    .captured();
    ui.say(&Report::new(Icon::Refused, "short").code("E1801"));
    let line = cap
        .out()
        .lines()
        .next()
        .map(str::to_string)
        .unwrap_or_default();
    assert_eq!(line.chars().count(), 60);
    assert!(line.ends_with("E1801"));
    // too long to align: two spaces then the code
    let (ui, cap) = Ui::from_env(
        &flags(),
        &Env::default()
            .with("LANG", "en_US.UTF-8")
            .with("COLUMNS", "40"),
    )
    .captured();
    ui.say(
        &Report::new(
            Icon::Refused,
            "a very long headline that cannot fit beside a code",
        )
        .code("E1801"),
    );
    assert!(
        cap.out()
            .lines()
            .next()
            .unwrap_or("")
            .ends_with("beside a code  E1801")
            || cap.out().contains("  E1801")
    );
}

#[test]
fn json_shapes() {
    let (ui, cap) = Ui::from_env(
        &GlobalFlags {
            json: true,
            ..flags()
        },
        &tty(),
    )
    .captured();
    let r = Report::done("h")
        .detail("d")
        .next("n")
        .data(serde_json::json!({"k": 1}));
    assert_eq!(ui.finish("update", &Ok(r)), 0);
    let line = cap.out();
    assert!(line.starts_with("{\"ok\":true,\"command\":\"update\",\"headline\":\"h\",\"detail\":[\"d\"],\"next\":\"n\",\"code\":null,\"data\":{\"k\":1}}"), "{line}");
    assert_eq!(line.lines().count(), 1);
    let (ui, cap) = Ui::from_env(
        &GlobalFlags {
            json: true,
            ..flags()
        },
        &tty(),
    )
    .captured();
    let e = InstallerError::new("E1811", "no carton 1.4").with_subject("1.4");
    assert_eq!(ui.finish("carton", &Err(e)), 2);
    let v: serde_json::Value = serde_json::from_str(cap.out().trim()).unwrap_or_default();
    assert_eq!(v["ok"], false);
    assert_eq!(v["command"], "carton");
    assert_eq!(v["error"]["code"], "E1811");
    assert_eq!(v["error"]["kind"], "usage");
    assert_eq!(v["error"]["subject"], "1.4");
    assert!(v["error"]["hint"]
        .as_str()
        .is_some_and(|h| h.contains("ciginstaller carton list")));
    assert_eq!(cap.err(), "", "json writes nothing to stderr");
    // refused reports are not ok
    let (ui, cap) = Ui::from_env(
        &GlobalFlags {
            json: true,
            ..flags()
        },
        &tty(),
    )
    .captured();
    assert_eq!(
        ui.finish("check", &Ok(Report::new(Icon::Refused, "x").code("E1801"))),
        2
    );
    assert!(cap.out().starts_with("{\"ok\":false"));
}

#[test]
fn detection() {
    let base = Env::default().with("LANG", "en_US.UTF-8");
    assert_eq!(Ui::from_env(&flags(), &base).mode, Mode::Voice);
    assert_eq!(
        Ui::from_env(
            &GlobalFlags {
                plain: true,
                ..flags()
            },
            &base
        )
        .mode,
        Mode::Plain
    );
    assert_eq!(
        Ui::from_env(&flags(), &base.clone().with("CIG_PLAIN", "1")).mode,
        Mode::Plain
    );
    assert_eq!(
        Ui::from_env(&flags(), &base.clone().with("CIG_PLAIN", "0")).mode,
        Mode::Voice
    );
    assert_eq!(
        Ui::from_env(
            &GlobalFlags {
                plain: true,
                json: true,
                ..flags()
            },
            &base
        )
        .mode,
        Mode::Json
    );
    let t = |e: Env| Ui::from_env(&flags(), &Env { tty_out: true, ..e }).color;
    assert_eq!(
        t(base.clone().with("COLORTERM", "truecolor")),
        ColorMode::True
    );
    assert_eq!(t(base.clone().with("COLORTERM", "24bit")), ColorMode::True);
    assert_eq!(
        t(base.clone().with("TERM", "xterm-256color")),
        ColorMode::Ansi256
    );
    assert_eq!(t(base.clone().with("TERM", "xterm")), ColorMode::Ansi16);
    assert_eq!(
        t(base
            .clone()
            .with("COLORTERM", "truecolor")
            .with("NO_COLOR", "1")),
        ColorMode::None
    );
    assert_eq!(
        t(base
            .clone()
            .with("COLORTERM", "truecolor")
            .with("NO_COLOR", "")),
        ColorMode::True,
        "an empty NO_COLOR does not count"
    );
    assert_eq!(
        t(base
            .clone()
            .with("COLORTERM", "truecolor")
            .with("TERM", "dumb")),
        ColorMode::None
    );
    assert_eq!(
        Ui::from_env(&flags(), &base.clone().with("COLORFGBG", "0;15")).theme,
        Theme::Light
    );
    assert_eq!(
        Ui::from_env(&flags(), &base.clone().with("COLORFGBG", "15;0")).theme,
        Theme::Dark
    );
    assert_eq!(
        Ui::from_env(
            &GlobalFlags {
                theme: ThemeChoice::Light,
                ..flags()
            },
            &base
        )
        .theme,
        Theme::Light
    );
    assert_eq!(
        Ui::from_env(
            &GlobalFlags {
                theme: ThemeChoice::Dark,
                ..flags()
            },
            &base.clone().with("COLORFGBG", "0;15")
        )
        .theme,
        Theme::Dark
    );
    assert!(Ui::from_env(&flags(), &Env::default().with("LANG", "C")).ascii);
    assert!(
        Ui::from_env(
            &flags(),
            &Env::default()
                .with("LC_ALL", "C")
                .with("LANG", "en_US.UTF-8")
        )
        .ascii
    );
    assert!(Ui::from_env(&flags(), &base.clone().with("TERM", "dumb")).ascii);
    assert!(!Ui::from_env(&flags(), &base).ascii);
    assert_eq!(
        Ui::from_env(&flags(), &base.clone().with("COLUMNS", "100")).width,
        100
    );
    assert_eq!(Ui::from_env(&flags(), &base).width, 80);
}

#[test]
fn palette_roles_fall_back_by_depth() {
    use tokens::{ColorMode as C, Role as R, Theme as T};
    assert_eq!(
        sgr(C::True, T::Dark, R::Ember).as_deref(),
        Some("\x1b[38;2;249;115;22m")
    );
    assert_eq!(
        sgr(C::Ansi256, T::Dark, R::Ember).as_deref(),
        Some("\x1b[38;5;208m"),
        "ember is 208 on 256-colour terminals (#32)"
    );
    assert_eq!(
        sgr(C::Ansi16, T::Dark, R::Ember).as_deref(),
        Some("\x1b[93m"),
        "ember is bright yellow on 16 (#32)"
    );
    assert_eq!(sgr(C::Ansi16, T::Dark, R::Ok).as_deref(), Some("\x1b[32m"));
    assert_eq!(
        sgr(C::Ansi16, T::Dark, R::Warn).as_deref(),
        Some("\x1b[33m")
    );
    assert_eq!(
        sgr(C::Ansi16, T::Dark, R::Danger).as_deref(),
        Some("\x1b[31m")
    );
    assert_eq!(
        sgr(C::Ansi16, T::Dark, R::Ash400).as_deref(),
        Some("\x1b[2m")
    );
    assert_eq!(
        sgr(C::True, T::Light, R::Ember),
        sgr(C::True, T::Light, R::EmberInk),
        "the light theme's ember is ember-ink"
    );
    assert_ne!(sgr(C::True, T::Light, R::Ok), sgr(C::True, T::Dark, R::Ok));
    assert_eq!(sgr(C::None, T::Dark, R::Ok), None);
    assert_eq!(paint(C::None, T::Dark, R::Ok, "x"), "x");
    assert_eq!(paint(C::Ansi16, T::Dark, R::Ok, "x"), "\x1b[32mx\x1b[0m");
}

#[test]
fn tables_align_and_truncate() {
    let t = Table::new(&[
        ("name", Align::Left),
        ("n", Align::Right),
        ("what", Align::Left),
    ])
    .row(vec![
        "a".into(),
        "10".into(),
        "x".repeat(100).as_str().into(),
    ])
    .row(vec!["bbb".into(), "2".into(), "short".into()]);
    let (ui, _) = Ui::from_env(&flags(), &Env::default().with("LANG", "en_US.UTF-8")).captured();
    let text = ui.render_table(&t);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "  name    n   what");
    assert!(lines.iter().all(|l| l.chars().count() <= 80), "{text}");
    assert!(lines[1].ends_with('…'));
    assert_eq!(lines[2], "  bbb     2   short");
    assert!(ui
        .render_table(&t.clone().wide(true))
        .lines()
        .nth(1)
        .unwrap_or("")
        .contains(&"x".repeat(100)));
    let (ascii, _) = Ui::from_env(&flags(), &Env::default()).captured();
    assert!(ascii
        .render_table(&t)
        .lines()
        .nth(1)
        .unwrap_or("")
        .ends_with("..."));
}

#[test]
fn a_notice_is_at_most_one_per_command_and_never_in_json() {
    let (ui, cap) = Ui::from_env(&flags(), &Env::default().with("LANG", "en_US.UTF-8")).captured();
    ui.notice("one");
    ui.notice("two");
    assert_eq!(cap.err(), "· one\n");
    let (ui, cap) = Ui::from_env(
        &GlobalFlags {
            json: true,
            ..flags()
        },
        &Env::default(),
    )
    .captured();
    ui.notice("one");
    assert_eq!(cap.err(), "");
}

#[test]
fn results_go_to_stdout_and_chatter_to_stderr() {
    let (ui, cap) = Ui::from_env(&flags(), &Env::default().with("LANG", "en_US.UTF-8")).captured();
    ui.say(&Report::new(Icon::Running, "working"));
    ui.say(&Report::done("finished"));
    assert_eq!(
        (cap.out().as_str(), cap.err().as_str()),
        ("✓ finished\n", "● working\n")
    );
    let (ui, cap) = Ui::from_env(
        &GlobalFlags {
            quiet: true,
            ..flags()
        },
        &Env::default(),
    )
    .captured();
    ui.say(&Report::new(Icon::Running, "working"));
    ui.say(&Report::new(Icon::Note, "n"));
    ui.say(&Report::done("finished"));
    assert_eq!(cap.err(), "");
    assert!(cap.out().contains("finished"));
}

// ---- the registry -----------------------------------------------------------------------

const SPEC_CODES: [(u32, &str); 55] = [
    (1800, "combination refused"),
    (1801, "a product needs another"),
    (1802, "a stick does not fit its carton"),
    (1803, "yanked version or carton in use"),
    (1804, "security advisory"),
    (1805, "compiled program against installed cig"),
    (1806, "journal format against this cig"),
    (1807, "operating system or kernel below a minimum"),
    (1808, "a package's requirements"),
    (1810, "no manifest"),
    (1811, "no such carton"),
    (1812, "no such channel"),
    (1813, "no such pack"),
    (1814, "no such tool"),
    (1815, "no build for this platform"),
    (1816, "carton not installed"),
    (1820, "download failed"),
    (1821, "digest or size mismatch"),
    (1822, "trust floor not met"),
    (1823, "manifest expired"),
    (1824, "manifest sequence went backwards"),
    (1825, "manifest malformed or format too new"),
    (1826, "unsafe archive"),
    (1827, "sidecar missing or disagrees"),
    (1828, "attestation failed"),
    (1830, "an owned file is unreadable or too new"),
    (1831, "installed file differs from its receipt"),
    (1832, "an interrupted change was recovered"),
    (1833, "rollback incomplete"),
    (1834, "another ciginstaller holds the lock"),
    (1835, "home directory unsafe or unwritable"),
    (1836, "disk full"),
    (1840, "nothing installed for this tool"),
    (1841, "pin file unreadable"),
    (1842, "pinned carton unavailable and fetching refused"),
    (1843, "no per-tool pin file for this tool"),
    (1844, "shim misnamed or shadowed"),
    (1845, "PATH block missing or hand-edited"),
    (1846, "pin target is a link or not a regular file"),
    (
        1847,
        "another pin file in this directory would outrank the one being written",
    ),
    (1848, "the edited pin file would not read back"),
    (1850, "downgrade refused"),
    (1851, "trial found differences and was declined"),
    (1852, "nothing to roll back to"),
    (1853, "self-update failed and the old binary was restored"),
    (1854, "update blocked by this project's pins"),
    (1860, "repair could not restore a file"),
    (1861, "--purge needs OS authentication"),
    (1862, "a profile block was edited and is left alone"),
    (1870, "usage"),
    (1871, "a question was needed and there is no TTY"),
    (1872, "platform not supported"),
    (1873, "offline was requested and the step needs the network"),
    (1890, "a bug in the installer"),
    (1891, "a journal could not be recovered"),
];

#[test]
fn the_registry_parses_and_has_every_spec_code() {
    let reg = registry::registry();
    assert_eq!(reg.version, 1);
    assert_eq!(reg.codes.len(), SPEC_CODES.len());
    for (n, title) in SPEC_CODES {
        let c = registry::lookup(&format!("E{n}")).unwrap_or_else(|| panic!("E{n} missing"));
        assert_eq!(c.title, title, "E{n}");
    }
    for c in &reg.codes {
        let n = number(&c.code).unwrap_or(0);
        assert!((1800..=1899).contains(&n), "{} outside E1800-E1899", c.code);
    }
}

/// The rules of cig's tests/registry.rs `every_code_has_a_complete_page`, applied here.
#[test]
fn every_code_has_a_complete_page() {
    const ARISES: [&str; 12] = [
        "parse", "check", "dry-run", "run", "light", "unburn", "runs", "doctor", "update", "crash",
        "compile", "any",
    ];
    const TAGS: [&str; 13] = [
        "beginner-likely",
        "ci-likely",
        "resource",
        "kernel",
        "fixable-by-doctor",
        "needs-burn",
        "irreversible-involved",
        "windows",
        "macos",
        "planned",
        "orchestrator",
        "compiler",
        "installer",
    ];
    let reg = registry::registry();
    let mut seen = BTreeSet::new();
    for c in &reg.codes {
        let id = &c.code;
        assert!(seen.insert(id.clone()), "{id} twice");
        assert!(
            !c.title.trim().is_empty() && c.meaning.trim().len() > 20,
            "{id}: title or meaning"
        );
        assert!(c.fix.trim().len() > 10, "{id}: fix too short");
        assert!(
            !c.fix.trim_start().starts_with(char::is_uppercase),
            "{id}: fix reads as a sentence about the problem"
        );
        assert!(
            registry::lookup_no(&c.no).is_some(),
            "{id}: unknown no {}",
            c.no
        );
        assert!(
            c.since.split('.').count() == 3 && c.since.split('.').all(|p| p.parse::<u32>().is_ok()),
            "{id}: since"
        );
        assert!(
            c.arises.iter().all(|a| ARISES.contains(&a.as_str())),
            "{id}: arises"
        );
        assert!(
            c.tags.iter().all(|t| TAGS.contains(&t.as_str())),
            "{id}: tags"
        );
        assert!(
            c.has_tag("installer"),
            "{id}: every entry is tagged installer"
        );
        assert!(
            c.related
                .iter()
                .all(|r| registry::lookup(r).is_some() && r != id),
            "{id}: related"
        );
        assert!(c.range().is_some(), "{id}: in no range");
        assert!(!c.causes.is_empty(), "{id}: no cause");
        for (i, cause) in c.causes.iter().enumerate() {
            assert!(
                cause.why.trim().len() > 8 && cause.remedy.trim().len() > 8,
                "{id} cause {i}"
            );
            assert!(
                registry::lookup_probe(&cause.probe).is_some(),
                "{id} cause {i}: unknown probe {}",
                cause.probe
            );
            assert!(
                cause
                    .no
                    .as_deref()
                    .is_none_or(|n| registry::lookup_no(n).is_some()),
                "{id} cause {i}: no"
            );
            assert!(
                !cause.why.contains("ciggies"),
                "{id} cause {i}: catchphrase in the text"
            );
        }
        if c.has_tag("fixable-by-doctor") {
            assert!(
                c.causes.iter().any(|k| k.fixable),
                "{id}: tagged fixable-by-doctor but no cause is"
            );
        }
    }
    let mut last = 0;
    for r in &reg.ranges {
        let (f, t) = (number(&r.from).unwrap_or(0), number(&r.to).unwrap_or(0));
        assert!(
            f < t && f > last,
            "range {}-{} overlaps or is out of order",
            r.from,
            r.to
        );
        last = t;
    }
    let probes: BTreeSet<_> = reg.probes.iter().map(|p| &p.name).collect();
    assert_eq!(probes.len(), reg.probes.len(), "duplicate probe");
    assert!(probes.contains(&"none".to_string()));
    for want in [
        "none",
        "cant-see-any",
        "never-heard-of-you",
        "these-are-mine",
        "dont-have-any",
        "depends",
    ] {
        assert!(registry::lookup_no(want).is_some(), "kind of no {want}");
    }
}

#[test]
fn the_no_block_is_cigs_verbatim() {
    let hear: Vec<&str> = registry::registry()
        .nos
        .iter()
        .map(|n| n.hear.as_str())
        .collect();
    for phrase in [
        "can't see any ciggies bro",
        "these are MY ciggies",
        "never heard of you",
        "don't have any over here",
    ] {
        assert!(hear.contains(&phrase), "{phrase}");
    }
}

#[test]
fn explain_every_code() {
    for c in registry::all() {
        let page = explain(&c.code).unwrap_or_default();
        assert!(page.starts_with(&c.code), "{}", c.code);
        assert!(
            page.contains("what to type next:")
                && page.contains("known causes")
                && page.contains("probe:")
                && page.contains("remedy:"),
            "{}",
            c.code
        );
        let plain = explain_with(&c.code, true).unwrap_or_default();
        assert!(
            !plain.contains("ciggies"),
            "{}: plain page has a catchphrase",
            c.code
        );
    }
    assert!(explain("e1811").is_some(), "lookup is case-insensitive");
    assert!(explain("E1899").is_none());
    assert!(explain("E501").is_none());
    let list = explain_list();
    assert!(list.contains("E1800-E1819") && list.contains("E1801") && list.contains("(planned)"));
}

#[test]
fn errors_get_kind_exit_and_catchphrase_from_the_registry() {
    let e = InstallerError::new("E1811", "m");
    assert_eq!((e.kind, e.exit), (Kind::Usage, 2));
    assert_eq!(InstallerError::new("E1820", "m").kind, Kind::Runtime);
    assert_eq!(InstallerError::new("E1820", "m").exit, 4);
    assert_eq!(InstallerError::new("E1870", "m").exit, 3);
    assert_eq!(
        (
            InstallerError::new("E1890", "m").kind,
            InstallerError::new("E1890", "m").exit
        ),
        (Kind::Internal, 70)
    );
    assert_eq!(InstallerError::new("E1831", "m").exit, 1);
    assert_eq!(
        registry::catchphrase("E1811"),
        Some("can't see any ciggies bro")
    );
    assert_eq!(registry::catchphrase("E1801"), Some("these are MY ciggies"));
    assert_eq!(registry::catchphrase("E1890"), None);
    assert_eq!(
        registry::catchphrase("E1820"),
        None,
        "depends has no catchphrase"
    );
    let (ui, cap) = Ui::from_env(&flags(), &Env::default().with("LANG", "en_US.UTF-8")).captured();
    ui.error(&InstallerError::new(
        "E1811",
        "there's no carton 1.4 on stable",
    ));
    let err = cap.err();
    assert!(err.starts_with(
        "✗ error[E1811 installer]: can't see any ciggies bro: there's no carton 1.4 on stable\n"
    ));
    assert!(
        err.contains("  = explain: ciginstaller explain E1811\n")
            && err.contains("  = fix: `ciginstaller carton list`\n")
    );
}
