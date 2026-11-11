// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `bench [--runs N] [--strict] [--scratch DIR] [--from DIR]` (#35): the installer's own budgets,
//! measured here and compared with `bench/budgets.toml`. The views that wait on other work
//! (the family, cartons against cartons, `--watch`, the window) say so with `E1867`.

use cigi_ui::{Align, Cell, Icon, InstallerError, Report, Table, EXIT_USAGE};

use super::tabular::details;
use super::Outcome;
use crate::cli::BenchCmd;
use crate::cx::Cx;
use crate::flows::bench::{self, Options, Status};

/// The flag asked for that is not built, and what it waits on, in #35's words.
fn unbuilt(c: &BenchCmd) -> Option<(&'static str, &'static str)> {
    let a = "it needs the suites `cig bench --family` runs: it depends on otmof-ops/cig-script#179 (the tool), which is open";
    if c.watch {
        return Some((
            "--watch",
            "it reruns a product's suite when its checkout changes, and the suites come from `cig bench`: it depends on otmof-ops/cig-script#179 (the tool), which is open",
        ));
    }
    if c.carton.is_some() {
        return Some(("--carton", a));
    }
    if !c.with.is_empty() {
        return Some(("--with", a));
    }
    if c.matrix.is_some() {
        return Some(("--matrix", a));
    }
    if c.product.is_some() {
        return Some(("--product", a));
    }
    if c.only.is_some() {
        return Some(("--only", a));
    }
    None
}

pub fn run(cx: &mut Cx, c: &BenchCmd) -> Result<Outcome, InstallerError> {
    if let Some((flag, why)) = unbuilt(c) {
        return Err(InstallerError::new(
            "E1867",
            format!("`bench {flag}` is not built yet: {why}"),
        )
        .with_fix("ciginstaller bench")
        .with_exit(EXIT_USAGE));
    }
    let opts = Options {
        runs: c.runs.unwrap_or(100).clamp(5, 5000),
        scratch: c.scratch.clone(),
        from: c.from.clone(),
    };
    let rows = bench::run_all(cx, &opts);
    let over: Vec<&bench::Row> = rows.iter().filter(|r| r.status == Status::Over).collect();
    let measured = rows
        .iter()
        .filter(|r| r.status != Status::NotMeasured)
        .count();
    let data = bench::json(&rows, &opts);

    if c.strict {
        if let Some(e) = bench::over_error(&rows) {
            return Err(e);
        }
    }

    let mut t = Table::new(&[
        ("budget", Align::Left),
        ("median", Align::Right),
        ("p95", Align::Right),
        ("limit", Align::Right),
        ("", Align::Left),
    ]);
    for r in &rows {
        let unit = &r.budget.unit;
        let cell = |v: Option<f64>| v.map_or("-".to_string(), |v| bench::show(v, unit));
        let verdict = if r.status == Status::NotMeasured || r.status == Status::Info {
            format!("{}: {}", r.status.word(), r.note)
        } else {
            r.status.word().to_string()
        };
        t = t.row(vec![
            Cell::from(r.budget.what.clone()),
            Cell::new(cell(r.median)),
            Cell::new(cell(r.p95)),
            Cell::new(bench::show(r.budget.limit, unit)),
            if r.status == Status::Over {
                Cell::changed(verdict)
            } else {
                Cell::new(verdict)
            },
        ]);
    }
    let headline = if over.is_empty() {
        format!("{measured} budget(s) measured here, none over")
    } else {
        format!("{measured} budget(s) measured here, {} over", over.len())
    };
    let icon = if over.is_empty() {
        Icon::Done
    } else {
        Icon::Note
    };
    let mut r = Report::new(icon, headline);
    for l in details(&cx.ui, &t) {
        r = r.detail(l);
    }
    if cfg!(debug_assertions) {
        r = r.detail("this is a debug build: the sizes are not judged and the timings are slower than a release build's");
    }
    r = r.detail(format!(
        "{} run(s) of the shim; measured here, read-only, and nothing is sent anywhere",
        opts.runs
    ));
    r = r.detail(
        "not built yet: the family view, cartons against cartons, --watch and the window's Bench view; ciginstaller explain E1867 says what each waits on",
    );
    if !over.is_empty() {
        r = r.next("ciginstaller explain E1866");
    } else if cx.state.cartons.is_empty() {
        r = r.next("ciginstaller install carton");
    }
    Ok(r.data(data).into())
}
