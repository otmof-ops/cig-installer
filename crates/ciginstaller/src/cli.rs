// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0
//! The command line (SPEC 10), as far as this phase builds it.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "ciginstaller",
    about = "Installs, pins, updates, repairs and removes the CigScript family",
    version,
    disable_help_subcommand = true
)]
pub struct Cli {
    /// One JSON object per result on stdout.
    #[arg(long, global = true)]
    pub json: bool,
    /// The same facts without decoration.
    #[arg(long, global = true)]
    pub plain: bool,
    /// Never use colour (also NO_COLOR).
    #[arg(long, global = true)]
    pub no_color: bool,
    /// Take the safe default of every question, and yes to the reversible ones.
    #[arg(long, short = 'y', global = true)]
    pub yes: bool,
    /// Also say yes to irreversible steps.
    #[arg(long, global = true)]
    pub yes_irreversible: bool,
    /// Only results and errors.
    #[arg(long, short = 'q', global = true)]
    pub quiet: bool,
    /// More detail.
    #[arg(long, short = 'v', global = true)]
    pub verbose: bool,
    /// Use only what is cached; no request is made.
    #[arg(long, global = true)]
    pub offline: bool,
    /// The home directory (same as CIGSCRIPT_HOME).
    #[arg(long, global = true, value_name = "DIR")]
    pub home: Option<PathBuf>,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    /// What an error code means, why it comes and what fixes it; every code with none.
    Explain {
        /// A code such as E1801.
        code: Option<String>,
    },
    /// Install a carton, a pack of it, or one tool of it, verified, as one transaction.
    Install(InstallCmd),
    /// Which carton this machine is on, or move it; `carton list` and `carton show <v>`.
    Carton(CartonCmd),
    /// Make an installed carton the machine's (no download).
    #[command(name = "use")]
    Use {
        carton: String,
        #[arg(long)]
        dry_run: bool,
    },
    /// Write a project pin file.
    Pin {
        /// `carton` (`.cig-carton`) or `cig` (`.cig-version`).
        tool: String,
        /// A pin: `0.1`, `0.1.2` or `>=0.1.2`.
        pin: String,
        /// Write the key in `cig-tasks.json` or `cigar.toml` instead.
        #[arg(long, value_name = "FILE")]
        file: Option<String>,
    },
    /// Installed cartons, sticks and tools.
    List,
    /// What runs here, and why.
    Which {
        tool: String,
        /// The resolver's steps.
        #[arg(long)]
        why: bool,
        /// Only the path.
        #[arg(long)]
        path: bool,
        /// Resolve as if run from this directory.
        #[arg(long, value_name = "DIR")]
        cwd: Option<PathBuf>,
    },
    #[command(name = "__resolve", hide = true)]
    Resolve {
        #[arg(long)]
        tool: String,
        #[arg(long)]
        cwd: PathBuf,
        #[arg(last = true)]
        args: Vec<std::ffi::OsString>,
    },
}

#[derive(Args)]
pub struct InstallCmd {
    #[command(subcommand)]
    pub what: Option<InstallWhat>,
    /// An exact carton or a pin such as 0.1 (default: the newest, or the machine's for a pack or stick).
    #[arg(long, global = true, value_name = "V")]
    pub carton: Option<String>,
    /// Do not touch shell profiles.
    #[arg(long, global = true)]
    pub no_path: bool,
    /// bash, zsh, fish, sh or powershell (default: from $SHELL).
    #[arg(long, global = true, value_name = "SHELL")]
    pub shell: Option<String>,
    /// A mirror directory to install from.
    #[arg(long, global = true, value_name = "DIR")]
    pub from: Option<PathBuf>,
    /// Raise the trust floor: pinned, attested or checksummed.
    #[arg(long, global = true, value_name = "LEVEL")]
    pub require: Option<String>,
    /// Install a yanked carton anyway.
    #[arg(long, global = true)]
    pub allow_yanked: bool,
    /// Print the plan and write nothing.
    #[arg(long, global = true)]
    pub dry_run: bool,
}

#[derive(Subcommand)]
pub enum InstallWhat {
    /// Every tool of the carton.
    Carton,
    /// A named set of tools of the carton.
    Pack {
        name: Option<String>,
        /// Pick tools by hand: a,b.
        #[arg(long, value_delimiter = ',')]
        pick: Vec<String>,
    },
    /// One tool of the carton.
    Stick { tool: String },
}

#[derive(Args)]
pub struct CartonCmd {
    #[command(subcommand)]
    pub sub: Option<CartonSub>,
    /// A carton to move this machine to (install it first when missing).
    pub version: Option<String>,
}

#[derive(Subcommand)]
pub enum CartonSub {
    /// Cartons on the channel and which are installed.
    List {
        #[arg(long)]
        all: bool,
    },
    /// What is inside one carton.
    Show { version: String },
}
