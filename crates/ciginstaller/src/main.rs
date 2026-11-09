// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0
//! `ciginstaller` (also `cigi`): installs, pins, updates, repairs and removes the CigScript
//! family. The commands arrive phase by phase (docs/decisions/0001-architecture.md); this
//! build has the error pages.

#![forbid(unsafe_code)]

// Fault injection (CIGI_FAULT aborts a transaction on purpose; CIGI_NOW sets the clock, which
// would defeat manifest expiry) is for debug test builds only. A release build that turned it on,
// through `--all-targets`, `cargo test --release` or a changed resolver, stops here.
#[cfg(all(feature = "fault-injection", not(debug_assertions)))]
compile_error!("the fault-injection feature is for debug test builds only, never a release build");

mod cli;
mod cmd;
mod cx;
mod errors;
mod flows;
mod project;
mod resolve;

use std::path::Path;

use cigi_ui::{GlobalFlags, InstallerError, Report, Ui, EXIT_SHIM, EXIT_USAGE};
use clap::Parser;

use cli::{Cli, Command};
use cmd::Outcome;
use cx::{Cx, Globals};

/// The names this binary answers to. Run under any other name, it is a misnamed shim.
fn our_name(argv0: &str) -> bool {
    let stem = Path::new(argv0)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    matches!(stem, "ciginstaller" | "cigi")
}

fn main() {
    let argv0 = std::env::args().next().unwrap_or_default();
    if !our_name(&argv0) {
        let ui = Ui::from_env_and_flags(&GlobalFlags::default());
        let e = InstallerError::new(
            "E1844",
            format!("`{argv0}` is ciginstaller under another name, not a shim"),
        )
        .with_fix("ciginstaller doctor")
        .with_exit(EXIT_SHIM);
        std::process::exit(ui.finish("shim", &Err(e)));
    }
    let cli = match Cli::try_parse() {
        Ok(c) => c,
        Err(e) => {
            let code = if e.use_stderr() { EXIT_USAGE } else { 0 };
            let _ = e.print();
            std::process::exit(code);
        }
    };
    let flags = GlobalFlags {
        json: cli.json,
        plain: cli.plain,
        no_color: cli.no_color,
        quiet: cli.quiet,
        yes: cli.yes,
        yes_irreversible: cli.yes_irreversible,
        verbose: cli.verbose,
        offline: cli.offline,
        home: cli.home.clone(),
        ..GlobalFlags::default()
    };
    let ui = Ui::from_env_and_flags(&flags);
    let ui = if matches!(cli.command, Some(Command::Resolve { .. })) {
        ui.all_to_stderr()
    } else {
        ui
    };
    std::process::exit(dispatch(cli, ui));
}

fn dispatch(cli: Cli, ui: Ui) -> i32 {
    let globals = Globals {
        yes: cli.yes,
        offline: cli.offline,
        home: cli.home.clone(),
    };
    let (name, cwd) = match &cli.command {
        None => ("ciginstaller", None),
        Some(Command::Explain { .. }) => ("explain", None),
        Some(Command::Install(_)) => ("install", None),
        Some(Command::Carton(_)) => ("carton", None),
        Some(Command::Channel { .. }) => ("channel", None),
        Some(Command::Use { .. }) => ("use", None),
        Some(Command::Pin { .. }) => ("pin", None),
        Some(Command::List) => ("list", None),
        Some(Command::Which { cwd, .. }) => ("which", cwd.clone()),
        Some(Command::Resolve { cwd, .. }) => ("resolve", Some(cwd.clone())),
    };
    if let Some(Command::Explain { code }) = &cli.command {
        return explain(&ui, code.as_deref(), cli.plain);
    }
    let result = (|| -> Result<Outcome, InstallerError> {
        if cli.command.is_none() {
            return Ok(Outcome::Report(
                Report::done("ciginstaller is installed").next("ciginstaller list"),
            ));
        }
        let mut cx = Cx::new(ui_clone(&cli), globals, cwd)?;
        match &cli.command {
            Some(Command::Install(c)) => cmd::install::run(&mut cx, c),
            Some(Command::Carton(c)) => cmd::carton::run(&mut cx, c),
            Some(Command::Channel { name }) => cmd::channel::run(&mut cx, name.as_deref()),
            Some(Command::Use { carton, dry_run }) => flows::use_carton::run(
                &mut cx,
                &flows::use_carton::UseArgs {
                    carton: carton.clone(),
                    dry_run: *dry_run,
                },
            )
            .map(Outcome::Report),
            Some(Command::Pin { tool, pin, file }) => {
                cmd::pin::run(&mut cx, tool, pin, file.as_deref())
            }
            Some(Command::List) => cmd::list::run(&mut cx),
            Some(Command::Which {
                tool, why, path, ..
            }) => cmd::which::run(&mut cx, tool, *why, *path),
            Some(Command::Resolve { tool, args, .. }) => {
                match cmd::resolve::run(&mut cx, tool, args) {
                    Ok(code) => Err(InstallerError::new("E1832", "").with_exit(code)),
                    Err(e) => Err(e),
                }
            }
            _ => Ok(Outcome::Report(Report::done("nothing to do"))),
        }
    })();
    match (name, result) {
        // the tool ran; its exit code is ours and nothing more is printed
        ("resolve", Err(e)) if e.code == "E1832" && e.message.is_empty() => e.exit,
        (_, Ok(Outcome::Page(text))) => {
            ui.page(name, &text);
            0
        }
        (_, Ok(Outcome::Report(r))) => ui.finish(name, &Ok(r)),
        (_, Err(e)) => ui.finish(name, &Err(e)),
    }
}

fn ui_clone(cli: &Cli) -> Ui {
    let ui = Ui::from_env_and_flags(&GlobalFlags {
        json: cli.json,
        plain: cli.plain,
        no_color: cli.no_color,
        quiet: cli.quiet,
        yes: cli.yes,
        yes_irreversible: cli.yes_irreversible,
        verbose: cli.verbose,
        offline: cli.offline,
        home: cli.home.clone(),
        ..GlobalFlags::default()
    });
    // on the shim's slow path stdout is the tool's: every word of ours goes to stderr
    if matches!(cli.command, Some(Command::Resolve { .. })) {
        ui.all_to_stderr()
    } else {
        ui
    }
}

fn explain(ui: &Ui, code: Option<&str>, plain: bool) -> i32 {
    match code {
        None => {
            ui.page("explain", &cigi_ui::explain_list());
            0
        }
        Some(c) => match cigi_ui::explain_with(&c.trim().to_ascii_uppercase(), plain) {
            Some(page) => {
                ui.page("explain", &page);
                0
            }
            None => {
                let e = InstallerError::new("E1870", format!("there is no error code `{c}`"))
                    .with_fix("ciginstaller explain")
                    .with_exit(EXIT_USAGE);
                ui.finish("explain", &Err(e))
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::our_name;

    #[test]
    fn answers_to_its_two_names_only() {
        assert!(our_name("/usr/local/bin/ciginstaller"));
        assert!(our_name("cigi"));
        assert!(!our_name("/home/a/.cigscript/bin/cigo"));
        assert!(!our_name("cig"));
    }
}
