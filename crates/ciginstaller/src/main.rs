// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `ciginstaller` (also `cigi`): installs, pins, updates, repairs and removes the CigScript
//! family. The commands arrive phase by phase (docs/decisions/0001-architecture.md); this
//! build has the error pages.

#![forbid(unsafe_code)]

// Fault injection (CIGI_FAULT aborts a transaction on purpose; CIGI_NOW sets the clock, which
// would defeat manifest expiry) is for debug test builds only. A release build that turned it on,
// through `--all-targets`, `cargo test --release` or a changed resolver, stops here.
#[cfg(all(feature = "fault-injection", not(debug_assertions)))]
compile_error!("the fault-injection feature is for debug test builds only, never a release build");

mod advisories;
mod cli;
mod cmd;
mod crash;
mod cx;
mod errors;
mod flows;
mod project;
mod resolve;
mod timings;

use std::path::Path;

use cigi_ui::{
    GlobalFlags, InstallerError, Report, ThemeChoice, Ui, VoiceChoice, EXIT_SHIM, EXIT_USAGE,
};
use clap::Parser;

use cli::{Cli, Command, SelfCmd, SelfSub};
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
    timings::begin();
    // a crash in the installer is written to the family ashtray from here on (#10, #28)
    crash::hook::install();
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
    timings::mark("start");
    let flags = flags_of(&cli);
    crash::hook::set_context(cli.home.clone(), cli.json, command_name(&cli.command));
    // the crash path's own test: a debug build only, never a release (see `crash::hook`)
    #[cfg(debug_assertions)]
    if let Ok(m) = std::env::var("CIGI_PLANT_PANIC") {
        if !m.is_empty() {
            panic!("{m}");
        }
    }
    let ui = Ui::from_env_and_flags(&flags);
    let ui = if matches!(cli.command, Some(Command::Resolve { .. })) {
        ui.all_to_stderr()
    } else {
        ui
    };
    let code = dispatch(cli, ui);
    timings::mark("output");
    timings::finish();
    std::process::exit(code);
}

fn dispatch(cli: Cli, ui: Ui) -> i32 {
    let globals = Globals {
        yes: cli.yes,
        offline: cli.offline,
        home: cli.home.clone(),
        self_updated: cli.self_updated.clone(),
    };
    let (name, cwd) = match &cli.command {
        None | Some(Command::Status) => ("status", None),
        Some(Command::Explain { .. }) => ("explain", None),
        Some(Command::Install(_)) => ("install", None),
        Some(Command::Carton(_)) => ("carton", None),
        Some(Command::Channel { .. }) => ("channel", None),
        Some(Command::Use { .. }) => ("use", None),
        Some(Command::Pin { .. }) => ("pin", None),
        Some(Command::List) => ("list", None),
        Some(Command::Licences { .. }) => ("licences", None),
        Some(Command::Which { cwd, .. }) => ("which", cwd.clone()),
        Some(Command::Resolve { cwd, .. }) => ("resolve", Some(cwd.clone())),
        Some(Command::Bundle { .. }) => ("bundle", None),
        Some(Command::Mirror { .. }) => ("mirror", None),
        // ---- the update lifecycle ----
        Some(Command::Update(_)) => ("update", None),
        Some(Command::Rollback { .. }) => ("rollback", None),
        // ---- end of the update lifecycle block ----
        // ---- health ----
        Some(Command::Doctor { .. }) => ("doctor", None),
        Some(Command::Repair { .. }) => ("repair", None),
        Some(Command::Uninstall { .. }) => ("uninstall", None),
        Some(Command::Remove { .. }) => ("remove", None),
        // ---- end of the health block ----
        Some(Command::SelfCmd(_)) => ("self", None),
        Some(Command::Gc { .. }) => ("gc", None),
        Some(Command::Check { project, .. }) => ("check", project.clone()),
        Some(Command::SelfCheck) => ("__selfcheck", None),
        // ---- status, settings, history, shims (a4) ----
        Some(Command::Config(_)) => ("config", None),
        Some(Command::History) => ("history", None),
        Some(Command::Shims(_)) => ("shims", None),
        // ---- end of the a4 block ----
        Some(Command::Bench(_)) => ("bench", None),
        // ---- crash (#10, #28) ----
        Some(Command::Crash(_)) => ("crash", None),
        Some(Command::Report(_)) => ("report", None),
        // ---- end of the crash block ----
        Some(Command::Bug(_)) => ("bug", None),
        Some(Command::Programs(_)) => ("programs", None),
    };
    // the exit code a successful command ends with when it is not 0 (`update --check --exit-code`)
    let mut exit_override: Option<i32> = None;
    // kept after the command so `auto_update = notify` can look at the machine once it is done
    let mut notice_cx: Option<Cx> = None;
    if let Some(Command::Explain { code }) = &cli.command {
        return explain(&ui, code.as_deref());
    }
    // the crash commands read the ashtray and the config only: they work when the installer's
    // own state is the thing that is broken
    if let Some(Command::Crash(c)) = &cli.command {
        let result = cmd::crash::run(&ui, cli.home.as_ref(), c);
        return ui.finish("crash", &result);
    }
    // the report works when the installer's own state is what is broken: it opens `Cx` itself
    if let Some(Command::Report(r)) = &cli.command {
        let result = cmd::report::run(&ui, globals, cwd, r);
        return ui.finish("report", &result);
    }
    // the bug report works when the installer's own state is what is broken, as report does
    if let Some(Command::Bug(b)) = &cli.command {
        let result = cmd::bug::run(&ui, globals, cwd, b);
        return ui.finish("bug", &result);
    }
    let result = (|| -> Result<Outcome, InstallerError> {
        let mut cx = Cx::new(ui.duplicate(), globals, cwd)?;
        timings::mark("home");
        let result = match &cli.command {
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
            Some(Command::Licences { tool, text }) => {
                cmd::licences::run(&mut cx, tool.as_deref(), *text)
            }
            Some(Command::Which {
                tool,
                why,
                path,
                script,
                ..
            }) => cmd::which::run(&mut cx, tool.as_deref(), *why, *path, script.as_deref()),
            Some(Command::Resolve { tool, args, .. }) => {
                match cmd::resolve::run(&mut cx, tool, args) {
                    Ok(code) => Err(InstallerError::new("E1832", "").with_exit(code)),
                    Err(e) => Err(e),
                }
            }
            Some(Command::Bundle {
                carton,
                targets,
                out,
            }) => cmd::bundle::run(&mut cx, carton.as_deref(), targets, out),
            Some(Command::Mirror {
                dir,
                carton,
                channel,
                targets,
            }) => cmd::mirror::run(&mut cx, dir, carton.as_deref(), channel.as_deref(), targets),
            // ---- the update lifecycle ----
            Some(Command::Update(c)) if c.auto => flows::auto::run(
                &mut cx,
                &flows::auto::AutoArgs {
                    project: c.project.clone(),
                },
            )
            .map(Outcome::Report),
            Some(Command::Update(c)) => flows::update::run(&mut cx, &update_args(c)).map(|u| {
                if u.exit != 0 {
                    exit_override = Some(u.exit);
                }
                Outcome::Report(u.report)
            }),
            Some(Command::Rollback { tool, dry_run }) => flows::rollback::run(
                &mut cx,
                &flows::rollback::RollbackArgs {
                    tool: tool.clone(),
                    dry_run: *dry_run,
                },
            )
            .map(Outcome::Report),
            // ---- end of the update lifecycle block ----
            Some(Command::SelfCmd(SelfCmd {
                sub: SelfSub::Update { check },
            })) => cmd::self_cmd::update(&mut cx, *check),
            Some(Command::Gc { dry_run, keep_days }) => cmd::gc::run(&mut cx, *dry_run, *keep_days),
            // ---- health ----
            Some(Command::Doctor { network, tools }) => cmd::doctor::run(&mut cx, *network, *tools),
            Some(Command::Repair { tool, dry_run }) => {
                cmd::repair::run(&mut cx, tool.as_deref(), *dry_run)
            }
            Some(Command::Uninstall {
                tool,
                all,
                purge,
                keep_journal,
                dry_run,
            }) => cmd::uninstall::run(
                &mut cx,
                tool.as_deref(),
                *all,
                *purge,
                *keep_journal,
                *dry_run,
            ),
            Some(Command::Remove { carton, dry_run }) => {
                cmd::uninstall::remove(&mut cx, carton, *dry_run)
            }
            // ---- end of the health block ----
            Some(Command::Check { carton, .. }) => cmd::check::run(&mut cx, carton.as_deref()),
            Some(Command::SelfCheck) => cmd::self_cmd::selfcheck(&mut cx),
            // ---- status, settings, history, shims (a4) ----
            None | Some(Command::Status) => cmd::status::run(&mut cx),
            Some(Command::Config(c)) => cmd::config::run(&mut cx, c.sub.as_ref()),
            Some(Command::History) => cmd::history::run(&mut cx),
            Some(Command::Shims(c)) => cmd::shims::run(&mut cx, &c.sub),
            // ---- end of the a4 block ----
            Some(Command::Bench(c)) => cmd::bench::run(&mut cx, c),
            Some(Command::Programs(c)) => cmd::programs::run(&mut cx, c),
            _ => Ok(Outcome::Report(Report::done("nothing to do"))),
        };
        timings::mark("command");
        // the first good command of a freshly installed build says it is healthy (SPEC 12.4)
        if result.is_ok()
            && !matches!(
                cli.command,
                Some(Command::Resolve { .. } | Command::SelfCheck)
            )
            && !read_only(&cli.command)
            && !matches!(cli.command, Some(Command::Uninstall { .. }))
        {
            flows::self_update::mark_healthy(&cx);
        }
        notice_cx = Some(cx);
        result
    })();
    let ok = result.is_ok();
    let code = match (name, result) {
        // the tool ran; its exit code is ours and nothing more is printed
        ("resolve", Err(e)) if e.code == "E1832" && e.message.is_empty() => e.exit,
        (_, Ok(Outcome::Page(text))) => {
            ui.page(name, &text);
            0
        }
        (_, Ok(Outcome::Report(r))) => ui.finish(name, &Ok(r)),
        (_, Err(e)) => ui.finish(name, &Err(e)),
    };
    // `auto_update = notify`: at most one line a day, after an interactive command went well
    if ok
        && code == 0
        && !matches!(
            name,
            "update"
                | "rollback"
                | "resolve"
                | "explain"
                | "status"
                | "uninstall"
                | "doctor"
                | "licences"
                | "bench"
                | "crash"
                | "programs"
        )
    {
        if let Some(cx) = &notice_cx {
            flows::notice::maybe_notify(cx);
        }
    }
    // `auto_update = security | all` keeps a daily schedule; anything else takes it away. A
    // machine-locked value counts like any other. Quiet unless it changed something.
    if ok && code == 0 && !read_only(&cli.command) && name != "uninstall" && name != "resolve" {
        if let Some(cx) = notice_cx.as_mut() {
            flows::schedule::reconcile(cx);
        }
    }
    // `bench_after_update = on`: a short local sample once an update has switched the machine
    if ok
        && code == 0
        && name == "update"
        && ui.mode != cigi_ui::Mode::Json
        && !ui.quiet
        && !read_only(&cli.command)
    {
        if let Some(cx) = &notice_cx {
            flows::bench::after_update(cx);
        }
    }
    match (code, exit_override) {
        (0, Some(o)) => o,
        (c, _) => c,
    }
}

// ---- the update lifecycle ----
fn update_args(c: &cli::UpdateCmd) -> flows::update::UpdateArgs {
    flows::update::UpdateArgs {
        check: c.check,
        exit_code: c.exit_code,
        refresh: c.refresh,
        to: c.to.clone(),
        allow_downgrade: c.allow_downgrade,
        no_trial: c.no_trial,
        trial_only: c.trial_only,
        yes_differences: c.yes_differences,
        channel: c.channel.clone(),
        project: c.project.clone(),
        dry_run: c.dry_run,
    }
}
// ---- end of the update lifecycle block ----

/// A run that promises to write nothing (`--check`, `--dry-run`, `--trial-only`): it must not be
/// the one that marks the build healthy either (SPEC 12.2: `--check` writes only the manifest
/// cache).
fn read_only(cmd: &Option<Command>) -> bool {
    match cmd {
        Some(Command::Update(c)) => c.check || c.dry_run || c.trial_only,
        Some(Command::Install(c)) => c.dry_run,
        Some(Command::Doctor { .. }) => true,
        // they write a cache and the directory or file they were given, never the machine's state
        Some(Command::Bundle { .. } | Command::Mirror { .. }) => true,
        Some(Command::Bench(_)) => true,
        Some(Command::Crash(_) | Command::Report(_) | Command::Bug(_)) => true,
        // the list reads; a rebuild changes a program, not the machine's carton or its shims
        Some(Command::Programs(_)) => true,
        Some(Command::Licences { .. }) => true,
        Some(Command::Repair { dry_run, .. })
        | Some(Command::Remove { dry_run, .. })
        | Some(Command::Use { dry_run, .. })
        | Some(Command::Rollback { dry_run, .. })
        | Some(Command::Gc { dry_run, .. }) => *dry_run,
        Some(Command::SelfCmd(SelfCmd {
            sub: SelfSub::Update { check },
        })) => *check,
        _ => false,
    }
}

/// The command's name, as the crash hook's JSON error and the report name it.
fn command_name(cmd: &Option<Command>) -> &'static str {
    match cmd {
        None | Some(Command::Status) => "status",
        Some(Command::Install(_)) => "install",
        Some(Command::Update(_)) => "update",
        Some(Command::Crash(_)) => "crash",
        Some(Command::Report(_)) => "report",
        Some(Command::Bug(_)) => "bug",
        Some(Command::Programs(_)) => "programs",
        Some(Command::Doctor { .. }) => "doctor",
        Some(Command::Repair { .. }) => "repair",
        Some(Command::Uninstall { .. }) => "uninstall",
        Some(Command::SelfCmd(_)) => "self",
        _ => "",
    }
}

/// The global flags, with `theme` and `voice` read from `installer/config` (#31, #32).
fn flags_of(cli: &Cli) -> GlobalFlags {
    let over = cli.home.clone().map(std::path::PathBuf::into_os_string);
    let home = cigi_home::Home::discover_with(&|k| match (k, &over) {
        ("CIGSCRIPT_HOME", Some(h)) => Some(h.clone()),
        _ => std::env::var_os(k),
    });
    let cfg = home.ok().and_then(|h| cigi_home::Config::load(&h).ok());
    let get = |k: &str| cfg.as_ref().map(|c| c.get(k)).unwrap_or_default();
    GlobalFlags {
        json: cli.json,
        plain: cli.plain,
        no_color: cli.no_color,
        quiet: cli.quiet,
        yes: cli.yes,
        yes_irreversible: cli.yes_irreversible,
        verbose: cli.verbose,
        offline: cli.offline,
        home: cli.home.clone(),
        theme: ThemeChoice::parse(&get("theme")).unwrap_or_default(),
        voice: VoiceChoice::parse(&get("voice")).unwrap_or_default(),
        wide: cli.wide,
        // the shim's slow path must never wait on the terminal
        no_probe: matches!(cli.command, Some(Command::Resolve { .. })),
    }
}

fn explain(ui: &Ui, code: Option<&str>) -> i32 {
    match code {
        None => {
            ui.page("explain", &cigi_ui::explain_list());
            0
        }
        Some(c) => match cigi_ui::explain_with(&c.trim().to_ascii_uppercase(), ui.plain()) {
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
