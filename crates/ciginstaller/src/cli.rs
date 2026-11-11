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
    /// The same facts in neutral words. Also automatic under CI (`CI` set), a screen reader
    /// (`CIG_SCREEN_READER=1`), a non-English locale, and `voice = plain` in the installer's config.
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
    /// The license of the installer and of each build here, and where its text is; `--text`
    /// prints one license file. Reads this machine only; nothing is sent anywhere.
    Licences {
        /// Only this one: `ciginstaller` or a tool.
        tool: Option<String>,
        /// Print the license text of `tool` as it is.
        #[arg(long)]
        text: bool,
    },
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
        /// For `cig`: as if running this script, so a `# cig >=1.6.2` header in it counts.
        #[arg(long, value_name = "FILE")]
        script: Option<PathBuf>,
    },
    // ---- offline: bundle, mirror ----
    /// One file with a carton's manifest and the builds of chosen targets, for a machine with no
    /// network. Unsigned until #9; install it with `install --from <file>`.
    Bundle {
        /// An exact carton or a pin such as 0.1 (default: the newest that is not yanked).
        #[arg(long, value_name = "V")]
        carton: Option<String>,
        /// Targets to include, such as linux-x86_64,windows-x86_64 (default: this machine's).
        #[arg(long, value_delimiter = ',', value_name = "TARGET")]
        targets: Vec<String>,
        /// The file to write.
        #[arg(long, value_name = "FILE")]
        out: PathBuf,
    },
    /// Fill a directory with the channel's manifest and builds, each admitted, so a team can
    /// serve it (`mirror = https://...`, or `install --from <dir>`). Run it again to bring it up
    /// to date; it removes nothing.
    Mirror {
        /// The directory to fill (made when missing).
        dir: PathBuf,
        /// Only this carton's builds (an exact carton or a pin); default every carton that is
        /// not yanked.
        #[arg(long, value_name = "V")]
        carton: Option<String>,
        /// The channel to mirror for this run (stable, beta or nightly).
        #[arg(long, value_name = "CHANNEL")]
        channel: Option<String>,
        /// Only these targets (default: every target the builds are published for).
        #[arg(long, value_delimiter = ',', value_name = "TARGET")]
        targets: Vec<String>,
    },
    // ---- end of the offline block ----
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
    // ---- bench (#35) ----
    /// Measure this machine against the performance budgets; nothing is sent anywhere.
    Bench(BenchCmd),
    // ---- end of the bench block ----
    // ---- crash (#10, #28) ----
    /// The family's crash reports, every tool's together: `list`, `show` (exactly what would be
    /// sent), `send`, `delete`, and `submit` for a tool handing one in.
    Crash(CrashCmd),
    /// What a bug report needs in one redacted bundle: the version block (the installer, the
    /// carton, each tool's build, what the project pins, mismatches flagged), the operation's
    /// journal and the trial's differences, or cig's own bundle for a cig run. Nothing is
    /// written or sent until `--out` or `--send` says so.
    Report(ReportCmd),
    // ---- end of the crash block ----
    // ---- bug (#29) ----
    /// Write up what went wrong, with the evidence for it, and see exactly what would be sent
    /// before anything is saved. Asked in the voice on a terminal; without one, nothing is asked
    /// and the answers come from flags. `bug list` and `bug show <n>` are the reports saved here.
    Bug(BugCmd),
    // ---- end of the bug block ----
    // ---- programs (#30) ----
    /// The compiled programs on this machine, found by their trailer on `PATH` and in this
    /// project's folders (never a scan of the disk), the cig each was built for, the cigcompile
    /// that made it and any advisory that covers that cig. `programs rebuild <name>` rebuilds
    /// one from its script with the carton's cigcompile. Nothing is sent anywhere.
    Programs(ProgramsCmd),
    // ---- end of the programs block ----
    // ---- health: doctor, repair, uninstall, remove ----
    /// One report on the whole family: shims, PATH, toolchains, state, matrix, manifest, disk,
    /// the tin, the guard's kernel features, humidor, transactions, and each tool's own doctor.
    /// Reads only; each finding ends with the command that fixes it.
    Doctor {
        /// Also fetch the manifest now (the only thing that uses the network).
        #[arg(long)]
        network: bool,
        /// Only the tools' own doctors, as they answer, under their headings. Without this the
        /// tools' doctors follow the installer's own findings.
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
    /// A mirror directory, or a bundle file made by `ciginstaller bundle`, to install from.
    #[arg(long, global = true, value_name = "DIR|FILE.tar")]
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
    /// The crash-report answer: always, never or ask. Recorded as the `reports` setting.
    #[arg(long, global = true, value_name = "WHEN")]
    pub reports: Option<String>,
    /// Every first-run answer in one JSON file, `{"preset": "strict", "reports": "never"}`, so a
    /// welcome screen can ask them itself and run the install with no terminal. `install
    /// --dry-run --json` lists the questions and their choices.
    #[arg(long, global = true, value_name = "FILE.json")]
    pub answers: Option<PathBuf>,
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
    /// What the background schedule runs: take the carton `auto_update` allows (`security`: one
    /// that clears a security advisory; `all`: the newest), through the trial, at most once a day
    /// and not on battery. A failed trial rolls back and is said on the next terminal.
    #[arg(
        long,
        conflicts_with_all = ["check", "to", "allow_downgrade", "no_trial", "trial_only", "yes_differences", "dry_run", "refresh", "exit_code"]
    )]
    pub auto: bool,
    /// Print the plan and write nothing.
    #[arg(long)]
    pub dry_run: bool,
}

// ---- bench (#35) ----
#[derive(Args)]
pub struct BenchCmd {
    /// How many shim dispatches to time (default 100, at least 5); the slower measurements take a fifth.
    #[arg(long, value_name = "N")]
    pub runs: Option<usize>,
    /// Fail with E1866 when any measurement is over its budget.
    #[arg(long)]
    pub strict: bool,
    /// Make the scratch home in this directory (default: the system's temporary directory).
    #[arg(long, value_name = "DIR")]
    pub scratch: Option<PathBuf>,
    /// Also time `install pack dev` and `install carton` from this local mirror, in a scratch home.
    #[arg(long, value_name = "DIR")]
    pub from: Option<PathBuf>,
    /// Not built yet (E1867): rerun when a product's checkout changes.
    #[arg(long)]
    pub watch: bool,
    /// Not built yet (E1867): this carton against another.
    #[arg(long, value_name = "V")]
    pub carton: Option<String>,
    /// Not built yet (E1867): one product swapped under the others, `cig=1.16.0`.
    #[arg(long = "with", value_name = "PRODUCT=V")]
    pub with: Vec<String>,
    /// Not built yet (E1867): cartons side by side, `carton=1.1.1,1.1.2`.
    #[arg(long, value_name = "LIST")]
    pub matrix: Option<String>,
    /// Not built yet (E1867): one product's suite.
    #[arg(long, value_name = "PRODUCT")]
    pub product: Option<String>,
    /// Not built yet (E1867): one case of a suite.
    #[arg(long, value_name = "CASE")]
    pub only: Option<String>,
}
// ---- end of the bench block ----

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

#[derive(Args)]
pub struct ReportCmd {
    /// The run: an operation of this machine (an id from `history`, or its transaction id), or
    /// a cig run id. The newest operation when omitted.
    pub run: Option<String>,
    /// The version block alone; with `--json`, the machine form a tool embeds in its own report.
    #[arg(long, conflicts_with_all = ["out", "send", "run", "cig"])]
    pub block: bool,
    /// The run is a cig run, not an operation of the installer.
    #[arg(long, requires = "run")]
    pub cig: bool,
    /// Write the bundle to this file.
    #[arg(long, value_name = "FILE", conflicts_with = "send")]
    pub out: Option<PathBuf>,
    /// File the bundle through the issues route, as `crash send` does, under the same consent
    /// (`reports = never` refuses); no identifier travels.
    #[arg(long)]
    pub send: bool,
}

#[derive(Args)]
pub struct BugCmd {
    #[command(subcommand)]
    pub sub: Option<BugSub>,
    /// The product it is about, as the family's messages name it: `ciginstaller bug cigar` is
    /// `--product cigar`.
    #[arg(value_name = "PRODUCT", conflicts_with_all = ["product", "run", "last"])]
    pub product_arg: Option<String>,
    /// It is about this run: an operation of this machine (an id from `history`) or a cig run.
    #[arg(long, value_name = "ID", conflicts_with_all = ["product", "last"])]
    pub run: Option<String>,
    /// It is about this product, and not a run: cig, cigo, cigcompile, program, cigar or installer.
    #[arg(long, value_name = "PRODUCT", conflicts_with = "last")]
    pub product: Option<String>,
    /// It is about the last thing that failed on this machine.
    #[arg(long)]
    pub last: bool,
    /// Which part, when it is not given by the run or product (cig, cigo, cigcompile, program,
    /// cigar, installer, unsure).
    #[arg(long, value_name = "PART")]
    pub part: Option<String>,
    /// What you were doing. Without a terminal this is how the question is answered.
    #[arg(long, value_name = "TEXT")]
    pub did: Option<String>,
    /// What you expected.
    #[arg(long, value_name = "TEXT")]
    pub expected: Option<String>,
    /// What happened instead.
    #[arg(long, value_name = "TEXT")]
    pub happened: Option<String>,
    /// Does it happen every time: yes, no or unsure.
    #[arg(long, value_name = "WORD")]
    pub every_time: Option<String>,
    /// Take a section out of the report (answers, environment, doctor, run, repro). The version
    /// matrix stays. Repeat it for more than one.
    #[arg(long, value_name = "SECTION")]
    pub without: Vec<String>,
    /// Keep the marked spans of your answers (a path, an address, an e-mail) as you typed them.
    /// A token-shaped span is always replaced. Without this, marked spans are replaced and listed.
    #[arg(long)]
    pub keep_marked: bool,
    /// Save the report here for sending later (`crash send`); nothing leaves. Without a terminal,
    /// neither this nor `--send` is implied by `--yes`: the preview is all that is printed.
    #[arg(long)]
    pub save: bool,
    /// Save the report and send it through the issues route under the crash command's consent
    /// (`reports = never` refuses).
    #[arg(long)]
    pub send: bool,
}

#[derive(Subcommand)]
pub enum BugSub {
    /// The bug reports saved on this machine, newest first: the receipt.
    List,
    /// One saved report, exactly as it travels: its number in `list`, or its id.
    Show {
        /// The number in `bug list`, or an id or a unique prefix of one.
        n: String,
    },
}

#[derive(Args)]
pub struct CrashCmd {
    #[command(subcommand)]
    pub sub: Option<CrashSub>,
}

#[derive(Subcommand)]
pub enum CrashSub {
    /// Every report in the ashtray, from every tool: its kind and whether it was filed.
    List,
    /// Print a report exactly as it would be sent. Nothing else travels.
    Show {
        /// Report id or a unique prefix.
        id: String,
    },
    /// Send a report to the issues route: the one yes for that report. `reports = never`
    /// (`CIG_REPORTS`, `CIGI_REPORTS`, the setting) refuses; with no route yet the report stays
    /// queued here and the output says so.
    Send {
        /// Report id or a unique prefix.
        id: Option<String>,
        /// Every report that is not filed yet.
        #[arg(long, conflicts_with = "id")]
        all: bool,
        /// Use only this route: `gh`, `api`, `link` (print the prefilled issue link; nothing is
        /// requested) or `file` (export, with `--out`). Without it: `gh`, then `api`, tried in order.
        #[arg(long, value_name = "ROUTE")]
        via: Option<String>,
        /// Where `--via file` writes the export (never overwrites).
        #[arg(long, value_name = "FILE", requires = "via")]
        out: Option<PathBuf>,
    },
    /// Try each route with a dry request (a search, issues only; nothing is filed) and say which
    /// can file a report now, one line per route. `reports = never` probes nothing.
    TestRoute {
        /// The repository to search (owner/name); the installer's own when omitted.
        #[arg(long, value_name = "OWNER/NAME")]
        repo: Option<String>,
    },
    /// The queue: what is waiting to leave, why, and each route's last success and failure.
    Status,
    /// Delete a report, or all of them.
    Delete {
        /// Report id or a unique prefix.
        id: Option<String>,
        /// Every saved report (asks first).
        #[arg(long, conflicts_with = "id")]
        all: bool,
    },
    /// Hand in a report another tool made (a superset of `cig crash submit`): it is anonymised
    /// again and joins the ashtray under the same consent.
    Submit {
        /// The tool that made it, such as cig-orchestrator.
        #[arg(long)]
        origin: String,
        /// The report, as JSON.
        #[arg(long, value_name = "FILE")]
        file: PathBuf,
        /// Where it goes (owner/name); the origin's repository when omitted.
        #[arg(long)]
        repo: Option<String>,
        /// Send it now, as `send` does.
        #[arg(long)]
        now: bool,
    },
}

// ---- programs (#30) ----

#[derive(Args)]
pub struct ProgramsCmd {
    #[command(subcommand)]
    pub sub: Option<ProgramsSub>,
    /// Also look in this project folder (and its `bin/`); may be repeated. The folder of the
    /// project you stand in is always looked in.
    #[arg(long, value_name = "DIR", global = true)]
    pub project: Vec<PathBuf>,
}

#[derive(Subcommand)]
pub enum ProgramsSub {
    /// Rebuild a program from its script with the carton's cigcompile on the installed cig, check
    /// it with `cigcompile verify` and `cigcompile test`, and swap it in as one transaction: the
    /// old program is replaced only once the new one has passed both.
    Rebuild {
        /// The program's name, or its path.
        target: String,
        /// The script, when it is not where the program says it was compiled from.
        #[arg(long, value_name = "FILE")]
        script: Option<PathBuf>,
        /// Say what would be rebuilt and replaced; build nothing.
        #[arg(long)]
        dry_run: bool,
    },
}
// ---- end of the programs block ----
