// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! Budgets (#32) at the `Ui` level: nothing built from an `Env` touches the terminal, the
//! first line of a report is rendered in far less than the budget, and the background query
//! cannot outlast its timeout. The first-line budgets for the real binary are measured in
//! `crates/ciginstaller/tests/budget.rs`.

use cigi_ui::detect::query_background;
use cigi_ui::tokens::budget;
use cigi_ui::*;
use std::time::{Duration, Instant};

#[test]
fn the_first_line_is_ready_far_inside_the_budget() {
    let env = Env {
        tty_out: true,
        tty_err: true,
        tty_in: true,
        ..Env::default()
    }
    .with("LANG", "en_US.UTF-8")
    .with("COLORTERM", "truecolor");
    let best = (0..20)
        .map(|_| {
            let start = Instant::now();
            let (ui, cap) = Ui::from_env(&GlobalFlags::default(), &env).captured();
            ui.say(&Report::done("carton 0.1.0: cig 1.20.0").next("ciginstaller update"));
            assert!(cap.out().starts_with('\x1b') || cap.out().starts_with('✓'));
            start.elapsed()
        })
        .min()
        .unwrap_or(Duration::MAX);
    assert!(
        best < Duration::from_millis(budget::STATUS_FIRST_LINE_MS),
        "building a Ui and drawing a line took {best:?}"
    );
}

#[test]
fn the_budgets_are_the_numbers_in_issue_32() {
    assert_eq!(budget::STATUS_FIRST_LINE_MS, 50);
    assert_eq!(budget::FIRST_LINE_MS, 100);
    assert_eq!(budget::BACKGROUND_QUERY_MS, 50);
}

#[cfg(unix)]
#[test]
fn the_background_query_without_a_terminal_answers_none_at_once() {
    // Only provable where there is no controlling terminal (CI, a container). On a developer's
    // terminal this would query it for real, so the check is skipped there; `examples/probe`
    // is how that case is tried by hand.
    if std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .is_ok()
    {
        return;
    }
    let start = Instant::now();
    assert_eq!(
        query_background(Duration::from_millis(budget::BACKGROUND_QUERY_MS)),
        None
    );
    assert!(start.elapsed() < Duration::from_millis(budget::BACKGROUND_QUERY_MS));
}

#[test]
fn nothing_built_from_an_env_queries_the_terminal() {
    // `from_env` is pure: even with every tty flag on and nothing said about the background,
    // it must come back at once and fall to the neutral theme.
    let env = Env {
        tty_out: true,
        tty_err: true,
        tty_in: true,
        ..Env::default()
    }
    .with("TERM", "xterm-256color");
    let start = Instant::now();
    let ui = Ui::from_env(&GlobalFlags::default(), &env);
    assert!(start.elapsed() < Duration::from_millis(5));
    assert_eq!(ui.theme, Theme::Neutral, "not found, so neutral");
    assert!(!ui.theme_known);
}
