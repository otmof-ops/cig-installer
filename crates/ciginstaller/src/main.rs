// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0
//! `ciginstaller` (also `cigi`): installs, pins, updates, repairs and removes the CigScript
//! family. The commands arrive phase by phase (docs/decisions/0001-architecture.md); this
//! build has the error pages.

#![forbid(unsafe_code)]

use std::path::Path;

use cigi_ui::{GlobalFlags, InstallerError, Report, Ui, EXIT_SHIM, EXIT_USAGE};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "ciginstaller",
    about = "Installs, pins, updates, repairs and removes the CigScript family",
    version,
    disable_help_subcommand = true
)]
struct Cli {
    /// One JSON object per result on stdout.
    #[arg(long, global = true)]
    json: bool,
    /// The same facts without decoration.
    #[arg(long, global = true)]
    plain: bool,
    /// Never use colour (also NO_COLOR).
    #[arg(long, global = true)]
    no_color: bool,
    /// Only results and errors.
    #[arg(long, short = 'q', global = true)]
    quiet: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// What an error code means, why it comes and what fixes it; every code with none.
    Explain {
        /// A code such as E1801.
        code: Option<String>,
    },
}

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
        ..GlobalFlags::default()
    };
    let ui = Ui::from_env_and_flags(&flags);
    let code = match cli.command {
        None => {
            let r = Report::done("ciginstaller is being built; `explain` is the first command")
                .next("ciginstaller explain");
            ui.finish("ciginstaller", &Ok(r))
        }
        Some(Command::Explain { code }) => explain(&ui, code.as_deref(), cli.plain),
    };
    std::process::exit(code);
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
