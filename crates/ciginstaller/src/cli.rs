// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! The command line.

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
    /// Never use color (also NO_COLOR).
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
    /// Tables are never cut to the terminal's width.
    #[arg(long, global = true)]
    pub wide: bool,
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
        /// `carton` (`.cig-carton`), `cig` (`.cig-version`), `cigo` (`.cigo-version`) or `cigar`
        /// (`.cigar-version`).
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
        /// A tool; with none, every tool of the family and why it runs from where it does.
        tool: Option<String>,
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
    /// Check, choose, verify, trial and switch to a newer carton.
    Update(UpdateCmd),
    /// Back to the previous carton, or one tool's build in it.
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
    // ---- status, settings, history, shims (a4) ----
    /// This machine at a glance: the carton, the tools, how they were trusted, what is waiting.
    Status,
    /// Settings: list them, `get <key>`, `set <key> <value>`, `unset <key>`.
    Config(ConfigCmd),
    /// What this machine has been through, newest first.
    History,
    /// The shims in `bin/`: `sync` rebuilds them and the index from state, `list` shows them.
    Shims(ShimsCmd),
    // ---- end of the a4 block ----
    // ---- health: doctor, repair, uninstall, remove ----
    /// One report on the whole family: shims, PATH, toolchains, state, matrix, manifest, disk,
    /// humidor, transactions. Reads only; each finding ends with the command that fixes it.
    Doctor {
        /// Also fetch the manifest now (the only thing that uses the network).
        #[arg(long)]
        network: bool,
        /// Print the tools' own doctors, as they answer, under their headings.
        #[arg(long)]
        tools: bool,
    },
    /// Check every installed file against its receipt and put back what is damaged, from the
    /// store or a fresh verified download; regenerate the shims and the index.
    Repair {
        /// Only this tool.
        #[arg(long, value_name = "TOOL")]
        tool: Option<String>,
        /// Say what would be done; change nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove one tool, or everything the installer added to this machine (`--all`); your data stays.
    Uninstall {
        /// The tool to remove.
        tool: Option<String>,
        /// Remove everything the family added: shims, toolchains, store, humidor entries, PATH
        /// blocks, and last `installer/`.
        #[arg(long)]
        all: bool,
        /// Also remove your data. Refused in this build.
        #[arg(long)]
        purge: bool,
        /// Keep the journal of `--all` beside the home instead of deleting it.
        #[arg(long)]
        keep_journal: bool,
        /// Say what would be removed; change nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove one installed carton and the builds nothing else uses.
    Remove {
        carton: String,
        /// Say what would be removed; change nothing.
        #[arg(long)]
        dry_run: bool,
    },
    // ---- end of the health block ----
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
    /// The security preset: chill, balanced, strict or paranoid. Recorded in config; the guard
    /// applies it once cig-orchestrator#66 lands.
    #[arg(long, global = true, value_name = "PRESET")]
    pub preset: Option<String>,
}

#[derive(Subcommand)]
pub enum InstallWhat {
    /// Every tool of the carton, optionally of an exact one: `install carton 1.2.0`.
    Carton {
        /// An exact carton or a pin such as 1.1 (the same as `--carton`; not both).
        version: Option<String>,
    },
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

// ---- status, settings, history, shims (a4) ----
#[derive(Args)]
pub struct ConfigCmd {
    #[command(subcommand)]
    pub sub: Option<ConfigSub>,
}

#[derive(Subcommand)]
pub enum ConfigSub {
    /// Every setting, its value and where the value comes from.
    List,
    /// One setting's value and where it comes from.
    Get { key: String },
    /// Set a setting in the config file (checked first; an environment variable still outranks it).
    Set { key: String, value: String },
    /// Remove a setting from the config file, back to its default.
    Unset { key: String },
}

#[derive(Args)]
pub struct ShimsCmd {
    #[command(subcommand)]
    pub sub: ShimsSub,
}

#[derive(Subcommand)]
pub enum ShimsSub {
    /// Rebuild every shim and the shim index from what is installed, in one transaction.
    Sync,
    /// Each shim and what it runs here.
    List,
}
// ---- end of the a4 block ----
