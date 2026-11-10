// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! `golden_output_per_screen`: each screen in dark, light, 16-colour, no-colour, plain and
//! JSON, compared byte for byte with `tests/golden/<screen>.<variant>.txt`. Escape bytes
//! are shown as `␛`. Regenerate with `UPDATE_GOLDEN=1 cargo test -p cigi-ui --test golden`
//! and read the diff before committing it.

use cig_compat::check::{Finding, Severity};
use cigi_ui::*;
use std::path::PathBuf;
use std::time::Duration;

fn variants() -> Vec<(&'static str, Ui, Capture)> {
    let tty = Env {
        tty_out: true,
        tty_err: true,
        tty_in: true,
        ..Env::default()
    }
    .with("LANG", "en_US.UTF-8")
    .with("TERM", "xterm-256color")
    // a dark terminal that has said so; the unknown background is tested in design.rs
    .with("COLORFGBG", "15;0");
    let piped = Env::default().with("LANG", "en_US.UTF-8");
    let flags = GlobalFlags::default();
    let mk = |name, ui: Ui| {
        let (ui, cap) = ui.captured();
        (name, ui, cap)
    };
    vec![
        mk(
            "dark",
            Ui::from_env(&flags, &tty.clone().with("COLORTERM", "truecolor")),
        ),
        mk(
            "light",
            Ui::from_env(
                &flags,
                &tty.clone()
                    .with("COLORTERM", "truecolor")
                    .with("COLORFGBG", "0;15"),
            ),
        ),
        mk(
            "ansi16",
            Ui::from_env(&flags, &tty.clone().with("TERM", "xterm")),
        ),
        mk(
            "no-color",
            Ui::from_env(&flags, &tty.clone().with("NO_COLOR", "1")),
        ),
        mk(
            "plain",
            Ui::from_env(
                &GlobalFlags {
                    plain: true,
                    ..flags.clone()
                },
                &piped,
            ),
        ),
        mk(
            "json",
            Ui::from_env(
                &GlobalFlags {
                    json: true,
                    ..flags.clone()
                },
                &piped,
            ),
        ),
        // LANG=C: the ASCII icons and glyphs, in the voice
        mk(
            "ascii",
            Ui::from_env(&flags, &Env::default().with("LANG", "C")),
        ),
    ]
}

/// `run` prints through the `Ui` and may return text from a pure renderer to record too.
fn check_golden(screen: &str, run: &dyn Fn(&Ui) -> String) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    for (name, ui, cap) in variants() {
        let rendered = run(&ui);
        let mut got = format!("--- stdout ---\n{}--- stderr ---\n{}", cap.out(), cap.err());
        if !rendered.is_empty() {
            got.push_str(&format!("--- rendered ---\n{rendered}"));
        }
        let got = got.replace('\x1b', "␛");
        let path = dir.join(format!("{screen}.{name}.txt"));
        if std::env::var_os("UPDATE_GOLDEN").is_some() {
            std::fs::write(&path, &got).unwrap();
            continue;
        }
        let want = std::fs::read_to_string(&path)
            .unwrap_or_else(|_| panic!("missing golden {}", path.display()));
        assert_eq!(got, want, "golden mismatch: {screen}.{name}");
    }
}

fn install_done() -> Report {
    Report::done("carton 0.1.0 installed: cig 1.20.0, cigo 1.17.0, cigcompile 0.2.0")
        .detail("trust: pinned 1, checksummed 2 (two sources agree); PATH block added to ~/.profile")
        .next("open a new shell, or run: export PATH=\"$HOME/.cigscript/bin:$PATH\"")
        .data(serde_json::json!({"carton": "0.1.0", "tools": {"cig": "1.20.0", "cigo": "1.17.0", "cigcompile": "0.2.0"}}))
}

fn finding() -> Finding {
    Finding {
        code: "E1801",
        severity: Severity::Refuse,
        headline: "cigo 1.4.1 needs cig 1.8.0 or newer; this project pins cig 1.7.2".into(),
        detail: vec![
            "checked: carton 0.1.0 (cig 1.20.0, cigo 1.17.0) against .cig-version in ~/work/site"
                .into(),
        ],
        fixes: vec![
            "ciginstaller pin carton 0.1".into(),
            "ciginstaller pin cig 1.8".into(),
        ],
        subject: "cigo".into(),
    }
}

fn carton_table() -> Table {
    Table::new(&[("product", Align::Left), ("installed", Align::Right), ("carton 0.1.1", Align::Right), ("changes", Align::Left)])
        .row(vec!["cig".into(), "1.20.0".into(), Cell::changed("1.20.1"), "programs refuse instead of turning into cig; the trial reports plan differences per script (#134-#137)".into()])
        .row(vec!["cigo".into(), "1.17.0".into(), "1.17.0".into(), "—".into()])
        .row(vec!["cigcompile".into(), "0.2.0".into(), "0.2.0".into(), "—".into()])
}

fn update_question() -> Question {
    Question::yes_no("update to carton 0.1.1?", true)
        .summary("cig 1.20.0 → 1.20.1, cigo unchanged")
        .detail("the trial ran clean. ciginstaller rollback undoes it.")
        .choice(Choice::new('s', "show changes"))
}

/// A prompt with its own choices, as the trial's differences screen asks.
fn trial_question() -> Question {
    let accept = Choice {
        key: 'a',
        label: "accept".into(),
        refusal: false,
        affirmative: true,
    };
    let keep = Choice {
        key: 'k',
        label: "keep 0.1.0".into(),
        refusal: true,
        affirmative: false,
    };
    Question {
        text: "the new cig plans 1 script differently".into(),
        summary: None,
        detail: vec![
            "~/work/site/deploy.cig   2 ops in the old plan, 3 in the new:".into(),
            "  + fs.mkdir  root0/d1            (new: the folder is created first now)".into(),
        ],
        choices: vec![accept, Choice::new('r', "review in full"), keep],
        default: 2,
        irreversible: false,
    }
}

fn pick(key: char, label: &str) -> Choice {
    Choice::new(key, label)
}

fn install_questions() -> Vec<Question> {
    let first = |mut q: Question| {
        q.choices[0].affirmative = true;
        q
    };
    vec![
        first(Question {
            text: "install the whole carton, or one stick?".into(),
            summary: Some(
                "the carton is cig, cigo, cigcompile and cigar, set to fit together".into(),
            ),
            detail: vec![
                "ciginstaller carton lists what fits; a stick is one tool on its own.".into(),
            ],
            choices: vec![pick('c', "carton"), pick('s', "stick")],
            default: 0,
            irreversible: false,
        }),
        first(Question {
            text: "which security preset?".into(),
            summary: Some("balanced".into()),
            detail: vec![
                "balanced asks before anything leaves the machine; chill asks less.".into(),
            ],
            choices: vec![
                pick('b', "balanced"),
                pick('c', "chill"),
                pick('s', "strict"),
                pick('p', "paranoid"),
            ],
            default: 0,
            irreversible: false,
        }),
        Question::yes_no("send crash reports?", false)
            .summary("only after you have read them")
            .detail("nothing is sent without a yes, each time."),
    ]
}

#[test]
fn golden_output_per_screen() {
    check_golden("install", &|ui| {
        ui.say(&Report::new(Icon::Running, "fetching carton 0.1.0"));
        ui.finish("install", &Ok(install_done()));
        String::new()
    });
    check_golden("which", &|ui| {
        let r = Report::done("cigo 1.17.0, from carton 0.1.0 (pinned in .cig-carton)")
            .detail("/home/u/.cigscript/toolchains/cigo/1.17.0/bin/cigo")
            .data(serde_json::json!({"tool": "cigo", "build": "1.17.0", "carton": "0.1.0", "path": "/home/u/.cigscript/toolchains/cigo/1.17.0/bin/cigo"}));
        ui.finish("which", &Ok(r));
        String::new()
    });
    check_golden("update", &|ui| {
        ui.say(&Report::new(
            Icon::Running,
            "checking manifest (stable) ...",
        ));
        ui.say(&Report::new(
            Icon::Note,
            "carton 0.1.0 → 0.1.1: cig 1.20.0 → 1.20.1   (cigo, cigcompile unchanged)",
        ));
        ui.table(&carton_table());
        ui.say(&Report::new(
            Icon::Running,
            "trial: cig on 14 scripts in ~/work/site",
        ));
        let r = Report::done(
            "carton 0.1.1 installed; trial: 14 scripts dry-ran the same on the new cig",
        )
        .next("nothing to do. ciginstaller rollback puts 0.1.0 back.");
        ui.finish("update", &Ok(r));
        ui.notice("carton 0.1.2 is out: ciginstaller update");
        ui.notice("this second notice is dropped");
        String::new()
    });
    check_golden("finding", &|ui| {
        ui.finding(&finding());
        ui.finish("check", &Ok(finding_report(&finding())));
        String::new()
    });
    check_golden("error-no-such-carton", &|ui| {
        let e = InstallerError::new("E1811", "there's no carton 1.4 on the stable channel")
            .with_hint("newest on stable is 0.1.1; 0.2.0-beta.2 is on beta")
            .with_fix("ciginstaller carton 0.1.1, or ciginstaller channel beta")
            .with_subject("1.4");
        assert_eq!(ui.finish("carton", &Err(e)), 2);
        String::new()
    });
    check_golden("error-bug", &|ui| {
        let e = InstallerError::new("E1890", "the shim index has two machine lines");
        assert_eq!(ui.finish("update", &Err(e)), 70);
        String::new()
    });
    check_golden("table", &|ui| {
        ui.table(&carton_table());
        String::new()
    });
    check_golden("table-wide", &|ui| {
        ui.table(&carton_table().wide(true));
        String::new()
    });
    check_golden("prompt", &|ui| ui.render_prompt(&update_question()));
    check_golden("prompt-irreversible", &|ui| {
        let q = Question::irreversible("remove everything ciginstaller added?")
            .summary("toolchains, store, shims, profile blocks");
        ui.render_prompt(&q)
    });
    check_golden("progress", &|ui| {
        let line = render_bar(
            ui,
            "fetching carton 0.1.0",
            Unit::Bytes,
            18_200_000,
            Some(31_000_000),
            Duration::from_secs(6),
        );
        let count = render_bar(
            ui,
            "trial: cig on your projects",
            Unit::Count("scripts"),
            9,
            Some(14),
            Duration::from_secs(4),
        );
        format!("{line}\n{count}\n")
    });
    check_golden("status", &|ui| {
        let r = Report::done("carton 0.1.0: cig 1.20.0, cigo 1.17.0, cigcompile 0.2.0")
            .detail("channel stable, trust pinned 1, checksummed 2")
            .detail("waiting: carton 0.1.1 (cig 1.20.0 → 1.20.1)")
            .next("ciginstaller update");
        ui.finish("status", &Ok(r));
        // #32: the status is at most 8 lines
        assert!(
            ui.render_report(&Report::done("x").detail("y").detail("z").next("n"))
                .lines()
                .count()
                <= 8
        );
        String::new()
    });
    check_golden("install-questions", &|ui| {
        install_questions()
            .iter()
            .map(|q| ui.render_prompt(q))
            .collect::<Vec<_>>()
            .join("\n")
    });
    check_golden("trial-differences", &|ui| {
        ui.render_prompt(&trial_question())
    });
    check_golden("which-why", &|ui| {
        let t = Table::new(&[
            ("step", Align::Left),
            ("looked at", Align::Left),
            ("found", Align::Left),
            ("result", Align::Left),
        ])
        .row(vec![
            "1".into(),
            "CIGI_CARTON".into(),
            "—".into(),
            "not set".into(),
        ])
        .row(vec![
            "2".into(),
            "~/work/site/.cig-version-cigo".into(),
            "—".into(),
            "no such file".into(),
        ])
        .row(vec![
            "3".into(),
            "~/work/site/.cig-carton".into(),
            "carton 0.1".into(),
            Cell::changed("used"),
        ])
        .row(vec![
            "4".into(),
            "machine carton".into(),
            "0.1.0".into(),
            "not needed".into(),
        ]);
        ui.table(&t);
        let r = Report::done("cigo 1.17.0, from carton 0.1.0 (pinned in .cig-carton)").data(
            serde_json::json!({"tool": "cigo", "build": "1.17.0", "carton": "0.1.0", "steps": 4}),
        );
        ui.finish("which", &Ok(r));
        String::new()
    });
    check_golden("doctor", &|ui| {
        let finding = |code, headline: &str, because: &str, fix: &str| Finding {
            code,
            severity: Severity::Warn,
            headline: headline.into(),
            detail: vec![because.into()],
            fixes: vec![fix.into()],
            subject: "doctor".into(),
        };
        ui.say(&Report::new(Icon::Note, "cig 1.20.0"));
        ui.finding(&finding(
            "E1845",
            "I think the PATH block is missing from your profile",
            "because I checked ~/.profile, ~/.bashrc and ~/.zshrc: none has the block",
            "ciginstaller repair",
        ));
        ui.say(&Report::new(Icon::Note, "cigcompile 0.2.0"));
        ui.finding(&finding(
            "E1806",
            "I think a program was built on an older cig",
            "because ~/work/tidy was built on cig 1.19.0 and cig 1.20.0 is installed",
            "cigcompile rebuild ~/work/tidy",
        ));
        ui.say(&Report::new(Icon::Note, "nothing else needed a look"));
        ui.finish(
            "doctor",
            &Ok(Report::done("2 findings, 2 fixes to run")
                .data(serde_json::json!({"findings": 2, "fixes": 2}))),
        );
        String::new()
    });
    check_golden("crash-list", &|ui| {
        let t = Table::new(&[
            ("id", Align::Left),
            ("when", Align::Left),
            ("tool", Align::Left),
            ("version", Align::Right),
            ("state", Align::Left),
        ])
        .row(vec![
            "c-0007".into(),
            "2026-11-09 21:14".into(),
            "cig".into(),
            "1.20.0".into(),
            Cell::changed("waiting for you to read it"),
        ])
        .row(vec![
            "c-0006".into(),
            "2026-11-02 08:40".into(),
            "cigcompile".into(),
            "0.2.0".into(),
            "sent, fixed in 0.2.1".into(),
        ]);
        ui.table(&t);
        ui.finish(
            "crash",
            &Ok(Report::done("2 reports; 1 waiting").next("ciginstaller crash show c-0007")),
        );
        String::new()
    });
    check_golden("crash-show", &|ui| {
        let a = ui.render_box(
            "what travels: the report",
            &[
                "ciginstaller 0.1.0, cig 1.20.0, linux x86_64".into(),
                "exit 70 in `cig run`, E502".into(),
            ],
        );
        let b = ui.render_box(
            "what stays: your files, paths and names",
            &["nothing from ~/work is included".into()],
        );
        ui.finish(
            "crash",
            &Ok(Report::done("c-0007: 2 sections travel, 1 stays")
                .next("ciginstaller crash send c-0007")),
        );
        format!("{a}{b}")
    });
    check_golden("bug", &|ui| {
        let q = Question {
            text: "does it happen every time?".into(),
            summary: None,
            detail: vec!["one question per screen; your answers are part of the preview.".into()],
            choices: vec![pick('y', "yes"), pick('n', "no"), pick('s', "sometimes")],
            default: 2,
            irreversible: false,
        };
        let preview = ui.render_box(
            "preview: this is exactly what is sent",
            &[
                "ciginstaller 0.1.0 on linux x86_64".into(),
                "update stopped at the trial; no files changed".into(),
            ],
        );
        let send = Question::yes_no("send it?", false)
            .detail("a public issue on otmof-ops/cig-installer; nothing else leaves the machine.");
        ui.finish(
            "bug",
            &Ok(
                Report::done("filed: https://github.com/otmof-ops/cig-installer/issues/41")
                    .data(serde_json::json!({"issue": 41})),
            ),
        );
        format!(
            "{}{preview}{}",
            ui.render_prompt(&q),
            ui.render_prompt(&send)
        )
    });
    check_golden("carton-list", &|ui| {
        let t = Table::new(&[
            ("carton", Align::Left),
            ("channel", Align::Left),
            ("cig", Align::Right),
            ("cigo", Align::Right),
            ("cigcompile", Align::Right),
            ("changes", Align::Left),
        ])
        .row(vec![
            Cell::changed("0.1.1"),
            "stable".into(),
            Cell::changed("1.20.1"),
            "1.17.0".into(),
            "0.2.0".into(),
            "programs refuse instead of turning into cig (#134-#137)".into(),
        ])
        .row(vec![
            "0.1.0".into(),
            "stable".into(),
            "1.20.0".into(),
            "1.17.0".into(),
            "0.2.0".into(),
            "installed".into(),
        ])
        .row(vec![
            "0.2.0-beta.2".into(),
            "beta".into(),
            "1.21.0".into(),
            "1.18.0".into(),
            "0.3.0".into(),
            "the passes, still in beta".into(),
        ]);
        ui.table(&t);
        let r = Report::done("3 cartons; 0.1.0 is installed")
            .data(serde_json::json!({"cartons": ["0.1.1", "0.1.0", "0.2.0-beta.2"], "installed": "0.1.0"}));
        ui.finish("carton", &Ok(r));
        String::new()
    });
    check_golden("programs", &|ui| {
        let t = Table::new(&[
            ("program", Align::Left),
            ("built with", Align::Right),
            ("cig now", Align::Right),
            ("advisory", Align::Left),
        ])
        .row(vec![
            "~/work/tidy".into(),
            Cell::changed("1.19.0"),
            "1.20.0".into(),
            "rebuild with cigcompile".into(),
        ])
        .row(vec![
            "~/work/site".into(),
            "1.20.0".into(),
            "1.20.0".into(),
            "—".into(),
        ]);
        ui.table(&t);
        ui.finish(
            "programs",
            &Ok(Report::done("2 programs; 1 needs a rebuild")
                .next("cigcompile rebuild ~/work/tidy")),
        );
        String::new()
    });
}
