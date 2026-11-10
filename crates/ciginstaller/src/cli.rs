// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
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
    /// Set by the installer itself when it starts its new build after a self-update: the version
    /// that build is. Only that run, of that build, skips the self-update step (one hop).
    #[arg(long, global = true, hide = true, value_name = "VERSION")]
    pub self_updated: Option<String>,
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
    /// The channel this machine follows, or switch it: the new channel's manifest is fetched
    /// and checked first; nothing is installed.
    Channel {
        /// stable, beta or nightly.
        name: Option<String>,
    },
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
    // ---- the update lifecycle: update, rollback ----
    /// Check, choose, verify, trial and switch to a newer carton (SPEC 12.2).
    Update(UpdateCmd),
    /// Back to the previous carton, or one tool's build in it (SPEC 12.3).
    Rollback {
        /// Switch only this tool's build (the machine becomes a mix, recorded as a stick).
        tool: Option<String>,
        /// Print the plan and write nothing.
        #[arg(long)]
        dry_run: bool,
    },
    // ---- end of the update lifecycle block ----
    /// Update ciginstaller itself.
    #[command(name = "self")]
    SelfCmd(SelfCmd),
    /// Remove cartons unused for `keep_days`, and the store objects nothing references.
    Gc {
        /// Say what would go and what it would free; change nothing.
        #[arg(long)]
        dry_run: bool,
        /// Remove what has been unused for longer than this many days (default: the `keep_days` setting).
        #[arg(long, value_name = "N")]
        keep_days: Option<u32>,
    },
    /// The marry-up matrix: this machine's combination, or one carton's.
    Check {
        /// Check this carton (an exact version or a pin) instead of the machine's.
        #[arg(long, value_name = "V")]
        carton: Option<String>,
        /// Also check the pins of the project in this directory.
        #[arg(long, value_name = "DIR")]
        project: Option<PathBuf>,
    },
    #[command(name = "__selfcheck", hide = true)]
    SelfCheck,
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
    /// Install from this channel for this run (stable, beta or nightly); the setting is unchanged.
    #[arg(long, global = true, value_name = "CHANNEL")]
    pub channel: Option<String>,
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

// ---- the update lifecycle ----
#[derive(Args)]
pub struct UpdateCmd {
    /// Only look: nothing is installed, fetched beyond the manifest, or written beyond its cache.
    #[arg(long)]
    pub check: bool,
    /// With --check: exit 10 when a newer carton exists.
    #[arg(long, requires = "check")]
    pub exit_code: bool,
    /// With --check: fetch the manifest even when the cached one is under a day old.
    #[arg(long, requires = "check")]
    pub refresh: bool,
    /// An exact carton or a pin such as 0.2 (default: the newest that is not yanked).
    #[arg(long, value_name = "CARTON")]
    pub to: Option<String>,
    /// Allow --to to name a carton older than the one installed.
    #[arg(long)]
    pub allow_downgrade: bool,
    /// Skip the trial of the new tools on this project.
    #[arg(long)]
    pub no_trial: bool,
    /// Run the trial and show the result; switch nothing and keep nothing.
    #[arg(long)]
    pub trial_only: bool,
    /// Accept a trial that found differences (`--yes` accepts identical results only).
    #[arg(long)]
    pub yes_differences: bool,
    /// Use this channel for this run.
    #[arg(long, value_name = "CHANNEL")]
    pub channel: Option<String>,
    /// The project the trial runs on and whose pins are respected (default: the working directory's).
    #[arg(long, value_name = "DIR")]
    pub project: Option<PathBuf>,
    /// Print the plan and write nothing.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Args)]
pub struct SelfCmd {
    #[command(subcommand)]
    pub sub: SelfSub,
}

#[derive(Subcommand)]
pub enum SelfSub {
    /// Update ciginstaller itself to the newest build on the channel.
    Update {
        /// Say whether a newer ciginstaller exists; change nothing.
        #[arg(long, alias = "dry-run")]
        check: bool,
    },
}
