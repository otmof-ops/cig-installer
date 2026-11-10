// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! The review fixes (F10 to F12 and the findings beside them): plain by locale, answers as
//! typed lines, three misses and out, the unknown background, doctor findings that need
//! action, bars that read without color, tables that are never cut off a terminal, and
//! the gates on the background query.

use cig_compat::check::{Finding, Severity};
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

fn unknown() -> Env {
    let mut e = tty();
    e.vars.remove("COLORFGBG");
    e
}

fn flags() -> GlobalFlags {
    GlobalFlags::default()
}

fn plain_flags() -> GlobalFlags {
    GlobalFlags {
        plain: true,
        ..flags()
    }
}

// ---- 1 and 12: plain is one resolved state ----------------------------------------------

#[test]
fn plain_is_the_uis_resolved_state_for_every_cause() {
    let plain = |f: GlobalFlags, e: &Env| Ui::from_env(&f, e).plain();
    assert!(!plain(flags(), &tty()));
    assert!(plain(plain_flags(), &tty()));
    assert!(plain(flags(), &tty().with("CI", "true")));
    assert!(plain(flags(), &tty().with("CIG_SCREEN_READER", "1")));
    assert!(plain(
        GlobalFlags {
            voice: VoiceChoice::Plain,
            ..flags()
        },
        &tty()
    ));
    assert!(plain(
        GlobalFlags {
            json: true,
            ..flags()
        },
        &tty()
    ));
}

#[test]
fn a_non_english_locale_gets_plain_until_a_voice_is_written() {
    let plain = |env: Env| Ui::from_env(&flags(), &env).plain();
    for english in [
        "en_AU.UTF-8",
        "en_US.UTF-8",
        "en_GB",
        "en",
        "C",
        "POSIX",
        "C.UTF-8",
    ] {
        assert!(!plain(tty().with("LANG", english)), "{english} is English");
    }
    for other in ["de_DE.UTF-8", "fr_FR", "ja_JP.UTF-8", "pt_BR.UTF-8", "es"] {
        assert!(plain(tty().with("LANG", other)), "{other} gets plain");
    }
    // LC_ALL beats LC_MESSAGES beats LANG
    assert!(plain(tty().with("LC_ALL", "de_DE.UTF-8")));
    assert!(plain(tty().with("LC_MESSAGES", "fr_FR.UTF-8")));
    assert!(!plain(
        tty()
            .with("LANG", "de_DE.UTF-8")
            .with("LC_ALL", "en_AU.UTF-8")
    ));
    // a character set alone says nothing about the language
    assert!(!plain(tty().with("LC_CTYPE", "de_DE.UTF-8")));
}

#[test]
fn explain_pages_follow_the_resolved_state() {
    let page = |f: GlobalFlags, e: &Env| {
        let ui = Ui::from_env(&f, e);
        explain_with("E1811", ui.plain()).expect("E1811 has a page")
    };
    assert!(page(flags(), &tty()).contains("can't see any ciggies bro"));
    for plain in [
        page(plain_flags(), &tty()),
        page(flags(), &tty().with("CI", "1")),
        page(flags(), &tty().with("CIG_SCREEN_READER", "1")),
        page(flags(), &tty().with("LANG", "de_DE.UTF-8")),
    ] {
        assert!(!plain.contains("ciggies"), "{plain}");
    }
}

// ---- 2 and 5: typed answers --------------------------------------------------------------

fn ask_with(q: &Question, f: GlobalFlags, inputs: &[&str]) -> (Answer, String) {
    let (ui, cap) = Ui::from_env(&f, &tty()).captured();
    for i in inputs {
        cap.push_input(i);
    }
    (ui.ask(q), cap.err())
}

#[test]
fn an_irreversible_question_takes_a_typed_yes_and_nothing_shorter() {
    let q = Question::irreversible("remove everything?");
    for yes in ["yes", "YES", "Yes", "  yes  "] {
        let (a, _) = ask_with(&q, flags(), &[yes]);
        assert!(a.is_yes(&q), "{yes:?}");
    }
    // `y` is not enough, and a single key never confirms
    let (a, err) = ask_with(&q, flags(), &["y", "yes"]);
    assert!(a.is_yes(&q), "the second try said yes");
    assert!(err.contains("type yes to go ahead"), "{err}");
    // no, n and enter are the default, which is no
    for no in ["no", "n", "N", ""] {
        let (a, _) = ask_with(&q, flags(), &[no]);
        assert!(!a.is_yes(&q), "{no:?}");
        assert_eq!(a.exit_code(), None);
    }
}

#[test]
fn the_irreversible_prompt_says_to_type_yes() {
    let (ui, _) = Ui::from_env(&flags(), &tty().with("NO_COLOR", "1")).captured();
    let p = ui.render_prompt(&Question::irreversible("remove everything?"));
    assert!(p.contains("type yes, then enter, to go ahead"), "{p}");
    assert!(p.contains("this can't be undone"), "{p}");
    assert!(!p.contains("[y]es"), "no one-key choices are offered: {p}");
}

#[test]
fn three_answers_that_are_not_options_stop_with_exit_3_and_take_no_default() {
    let q = Question::yes_no("update?", true);
    let (a, err) = ask_with(&q, flags(), &["x", "z", "?"]);
    assert_eq!(a.how, How::Aborted);
    assert_eq!(a.exit_code(), Some(3));
    assert!(!a.is_yes(&q), "a default of yes is not taken");
    assert_eq!(
        err.matches("pick one of: y n").count(),
        3,
        "each miss names the keys: {err}"
    );
    assert!(err.contains("stopped"), "it says so: {err}");
    // the same for an irreversible question, and for end of input
    let irr = Question::irreversible("sure?");
    let (a, _) = ask_with(&irr, flags(), &["y", "yep", "ok"]);
    assert_eq!((a.how, a.exit_code()), (How::Aborted, Some(3)));
    let (a, _) = ask_with(&q, flags(), &[]);
    assert_eq!(
        (a.how, a.exit_code()),
        (How::Aborted, Some(3)),
        "no input is not a yes"
    );
    // two misses then a good answer is fine
    let (a, _) = ask_with(&q, flags(), &["x", "z", "n"]);
    assert_eq!((a.how, a.key), (How::Typed, 'n'));
}

#[test]
fn under_plain_the_answers_are_typed_lines_naming_the_options() {
    let q = Question::yes_no("update?", true).choice(Choice::new('s', "show changes"));
    // a whole label works as well as its key
    let (a, _) = ask_with(&q, plain_flags(), &["show changes"]);
    assert_eq!(a.key, 's');
    let (a, _) = ask_with(&q, plain_flags(), &["NO"]);
    assert_eq!(a.key, 'n');
    // a stray word is not an option
    let (a, err) = ask_with(&q, plain_flags(), &["maybe", "later", "perhaps"]);
    assert_eq!(a.how, How::Aborted);
    assert!(err.contains("pick one of: y n s"), "{err}");
}

// ---- 4: an unknown background ------------------------------------------------------------

#[test]
fn an_unknown_background_leaves_text_alone_and_colors_only_glyphs() {
    let (ui, _) = Ui::from_env(&flags(), &unknown()).captured();
    assert_eq!(ui.theme, Theme::Neutral);
    let report = Report::done("carton 0.1.0 installed")
        .detail("trust: pinned 1")
        .next("ciginstaller list");
    let t = ui.render_report(&report);
    // the glyph and the arrow are painted, in the neutral palette (#16a34a, #ea580c)
    assert!(t.contains("\x1b[38;2;22;163;74m✓\x1b[0m"), "{t:?}");
    assert!(t.contains("\x1b[38;2;234;88;12m→\x1b[0m"), "{t:?}");
    // no word is colored: the rest of the text has no escape in it
    let words = t
        .replace("\x1b[38;2;22;163;74m✓\x1b[0m", "")
        .replace("\x1b[38;2;234;88;12m→\x1b[0m", "");
    assert!(!words.contains('\x1b'), "{words:?}");
    // an error's labels, a table's header and a code are text: unpainted
    let e = ui.render_error(&InstallerError::new("E1811", "no such carton").with_hint("try beta"));
    assert!(
        !e.contains("= hint:\x1b") && !e.contains("\x1b[0m try beta"),
        "{e:?}"
    );
    assert_eq!(
        e.matches('\x1b').count(),
        2,
        "only the ✗ glyph is painted: {e:?}"
    );
    let table = Table::new(&[("product", Align::Left)]).row(vec![Cell::changed("cig")]);
    assert!(!ui.render_table(&table).contains('\x1b'));
}

#[test]
fn an_unknown_background_in_16_colors_paints_glyphs_in_neutral_codes() {
    let env = unknown().with("TERM", "xterm").with("COLORTERM", "");
    let (ui, _) = Ui::from_env(&flags(), &env).captured();
    let t = ui.render_report(&Report::done("x").next("y"));
    assert!(t.contains("\x1b[94m✓\x1b[0m"), "ok is bright blue: {t:?}");
    assert!(
        t.contains("\x1b[95m→\x1b[0m"),
        "ember is bright magenta: {t:?}"
    );
}

#[test]
fn a_known_background_colors_text_as_before() {
    let (ui, _) = Ui::from_env(&flags(), &tty()).captured();
    let e = ui.render_error(&InstallerError::new("E1811", "no such carton").with_hint("try beta"));
    assert!(e.contains("\x1b[38;2;156;163;175m= hint:\x1b[0m"), "{e:?}");
}

// ---- 6: findings that need action ---------------------------------------------------------

fn finding(severity: Severity, fixes: &[&str]) -> Finding {
    Finding {
        code: "E1803",
        severity,
        headline: "carton 0.1.0 was yanked".into(),
        detail: vec![],
        fixes: fixes.iter().map(|f| (*f).to_string()).collect(),
        subject: "carton".into(),
    }
}

#[test]
fn a_finding_that_needs_action_is_warn_and_says_action_in_plain() {
    let act = finding(Severity::Warn, &["ciginstaller update"]);
    let note = finding(Severity::Note, &[]);
    let (plain, _) = Ui::from_env(&plain_flags(), &Env::default()).captured();
    assert!(plain
        .render_finding(&act)
        .starts_with("action: carton 0.1.0 was yanked"));
    assert!(plain
        .render_finding(&note)
        .starts_with("note: carton 0.1.0 was yanked"));
    // a warning with nothing to do is a note
    assert!(plain
        .render_finding(&finding(Severity::Warn, &[]))
        .starts_with("note:"));
    // the glyph stays #32's `·`, and the role is warn (#fbbf24 on dark)
    let (ui, _) = Ui::from_env(&flags(), &tty()).captured();
    let t = ui.render_finding(&act);
    assert!(t.starts_with("\x1b[38;2;251;191;36m·\x1b[0m"), "{t:?}");
    let (ascii, _) = Ui::from_env(&flags(), &Env::default().with("LANG", "C")).captured();
    assert!(ascii.render_finding(&act).starts_with("[-] carton"));
    assert_eq!(Icon::Action.plain_prefix(), "action:");
    assert_eq!(Icon::Action.role(), Role::Warn);
    assert_eq!(Icon::Action.glyph(false), Icon::Note.glyph(false));
    assert!(Icon::Action.is_ok());
}

// ---- 7: bars ---------------------------------------------------------------------------------

#[test]
fn the_fill_and_the_track_are_different_glyphs_in_every_mode() {
    use std::time::Duration;
    for (name, env) in [
        ("unicode", tty().with("NO_COLOR", "1")),
        ("ascii", Env::default().with("LANG", "C")),
        (
            "unicode, no color, piped",
            Env::default().with("LANG", "en_US.UTF-8"),
        ),
    ] {
        let (ui, _) = Ui::from_env(&flags(), &env).captured();
        let line = render_bar(
            &ui,
            "f",
            Unit::Count("x"),
            5,
            Some(10),
            Duration::from_secs(1),
        );
        let (fill, track) = if ui.ascii { ('#', '-') } else { ('━', '─') };
        assert!(
            line.contains(fill) && line.contains(track),
            "{name}: {line}"
        );
        assert_ne!(fill, track);
    }
    // colored, the same holds
    let (ui, _) = Ui::from_env(&flags(), &tty()).captured();
    let line = render_bar(
        &ui,
        "f",
        Unit::Count("x"),
        5,
        Some(10),
        Duration::from_secs(1),
    );
    assert!(line.contains('━') && line.contains('─'), "{line:?}");
}

// ---- 8: tables ------------------------------------------------------------------------------

#[test]
fn a_table_is_cut_only_for_a_person_at_a_terminal_in_the_voice() {
    let long = "y".repeat(200);
    let table = Table::new(&[("a", Align::Left), ("what", Align::Left)])
        .row(vec!["x".into(), long.as_str().into()]);
    let cut = |f: GlobalFlags, e: &Env| {
        let (ui, _) = Ui::from_env(&f, e).captured();
        !ui.render_table(&table).contains(&long)
    };
    assert!(
        cut(flags(), &tty()),
        "voice at a terminal is cut to the width"
    );
    assert!(
        !cut(flags(), &Env::default().with("LANG", "en_US.UTF-8")),
        "piped is never cut"
    );
    assert!(!cut(plain_flags(), &tty()), "plain is never cut");
    assert!(!cut(flags(), &tty().with("CI", "1")), "CI is plain");
    assert!(
        !cut(
            GlobalFlags {
                json: true,
                ..flags()
            },
            &tty()
        ),
        "json is never cut"
    );
    assert!(
        !cut(
            GlobalFlags {
                wide: true,
                ..flags()
            },
            &tty()
        ),
        "--wide is never cut"
    );
}

// ---- 9: the background query's gates -----------------------------------------------------

#[test]
fn the_background_query_is_off_when_anything_says_no() {
    let probe = |f: GlobalFlags, e: &Env, fg: bool| Ui::would_probe(&f, e, fg);
    assert!(probe(flags(), &unknown(), true), "the plain case probes");
    assert!(!probe(flags(), &unknown(), false), "not in the foreground");
    assert!(!probe(flags(), &tty(), true), "COLORFGBG already said");
    assert!(!probe(plain_flags(), &unknown(), true), "plain");
    assert!(!probe(flags(), &unknown().with("CI", "1"), true), "CI");
    assert!(!probe(
        flags(),
        &unknown().with("CIG_SCREEN_READER", "1"),
        true
    ));
    assert!(!probe(
        GlobalFlags {
            voice: VoiceChoice::Plain,
            ..flags()
        },
        &unknown(),
        true
    ));
    assert!(
        !probe(flags(), &unknown().with("LANG", "de_DE.UTF-8"), true),
        "non-English"
    );
    assert!(!probe(
        flags(),
        &unknown().with("SSH_CONNECTION", "1.2.3.4 22 5.6.7.8 22"),
        true
    ));
    assert!(!probe(
        flags(),
        &unknown().with("SSH_TTY", "/dev/pts/3"),
        true
    ));
    assert!(!probe(flags(), &unknown().with("NO_COLOR", "1"), true));
    assert!(!probe(flags(), &unknown().with("TERM", "dumb"), true));
    assert!(!probe(flags(), &unknown().with("TERM", "linux"), true));
    assert!(!probe(
        GlobalFlags {
            no_probe: true,
            ..flags()
        },
        &unknown(),
        true
    ));
    assert!(!probe(
        GlobalFlags {
            theme: ThemeChoice::Light,
            ..flags()
        },
        &unknown(),
        true
    ));
    let mut piped = unknown();
    piped.tty_out = false;
    assert!(!probe(flags(), &piped, true));
}

// ---- 13: the ASCII golden for every screen -----------------------------------------------

#[test]
fn every_screen_has_a_golden_in_every_variant() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let names: Vec<String> = std::fs::read_dir(&dir)
        .expect("the golden folder")
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .collect();
    let screens: std::collections::BTreeSet<&str> =
        names.iter().filter_map(|n| n.split('.').next()).collect();
    assert!(screens.len() >= 20, "{screens:?}");
    for screen in &screens {
        for variant in [
            "dark", "light", "ansi16", "no-color", "plain", "json", "ascii",
        ] {
            let file = format!("{screen}.{variant}.txt");
            assert!(names.contains(&file), "missing golden {file}");
        }
    }
    // the ASCII variant is ASCII
    for screen in &screens {
        let text =
            std::fs::read_to_string(dir.join(format!("{screen}.ascii.txt"))).expect("golden");
        let shown = text.replace('␛', "");
        // a golden records the product's output; the drawn glyphs must not appear in it
        for glyph in ['✓', '●', '✗', '·', '→', '━', '╸', '─', '—', '…'] {
            assert!(!shown.contains(glyph), "{screen}.ascii.txt has {glyph}");
        }
    }
}
