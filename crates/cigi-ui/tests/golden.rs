// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0

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
    .with("TERM", "xterm-256color");
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
        String::new()
    });
}
