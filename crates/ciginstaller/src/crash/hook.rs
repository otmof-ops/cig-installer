// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! The installer's own panic hook (#28 part 1, section 1; #10): a crash in ciginstaller is
//! written to the family ashtray, in cig's report schema, with the origin `ciginstaller`, the
//! moment it happens. Nothing is sent: a report leaves only through `crash send`.
//!
//! The hook never panics (a second panic while writing is ignored, and a failure to write is
//! said, not raised), and it runs under `panic = "abort"` too: the hook is called before the
//! process aborts.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use super::{anon, new_report, write_new, Kind, Report};

#[derive(Default)]
struct Context {
    home: Option<PathBuf>,
    json: bool,
    command: String,
}

fn context() -> &'static Mutex<Context> {
    static C: OnceLock<Mutex<Context>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(Context::default()))
}

static BUSY: AtomicBool = AtomicBool::new(false);

/// What the hook needs to know once the command line is parsed: `--home`, `--json`, the command.
pub fn set_context(home: Option<PathBuf>, json: bool, command: &str) {
    if let Ok(mut c) = context().lock() {
        c.home = home;
        c.json = json;
        c.command = command.to_string();
    }
}

/// Install the hook. Call it first thing in `main`.
pub fn install() {
    std::panic::set_hook(Box::new(|info| {
        if BUSY.swap(true, Ordering::SeqCst) {
            return;
        }
        let message = match info.payload().downcast_ref::<&str>() {
            Some(s) => (*s).to_string(),
            None => info
                .payload()
                .downcast_ref::<String>()
                .cloned()
                .unwrap_or_else(|| "(no message)".to_string()),
        };
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", crate_relative(l.file()), l.line(), l.column()))
            .unwrap_or_else(|| "?".to_string());
        let backtrace = std::backtrace::Backtrace::force_capture().to_string();
        let _ = std::panic::catch_unwind(|| record(&message, &location, &backtrace));
    }));
}

/// The subcommands and flags the parser knows: the words a command line may carry in a report.
pub fn cli_words() -> Vec<String> {
    use clap::CommandFactory;
    fn walk(c: &clap::Command, out: &mut Vec<String>) {
        out.push(c.get_name().to_string());
        for a in c.get_arguments() {
            out.extend(a.get_long().map(str::to_string));
            out.extend(a.get_short().map(|s| s.to_string()));
        }
        for s in c.get_subcommands() {
            walk(s, out);
        }
    }
    let mut out = Vec::new();
    walk(&crate::cli::Cli::command(), &mut out);
    out
}

fn record(message: &str, location: &str, backtrace: &str) {
    anon::know_words(cli_words());
    let (home, json, command) = context()
        .lock()
        .map(|c| (c.home.clone(), c.json, c.command.clone()))
        .unwrap_or_default();
    let root = cigi_home::Home::for_running(home.as_deref())
        .ok()
        .map(|h| h.root());
    let mut report = new_report(
        super::ORIGIN,
        env!("CARGO_PKG_VERSION"),
        Kind::Crash,
        location,
        message,
        filter_backtrace(backtrace),
        root.as_deref(),
    );
    let saved = root.as_deref().map(|r| write_new(r, &mut report));
    let (id, path, why) = match saved {
        Some(Ok(p)) => (Some(report.id.clone()), Some(p), None),
        Some(Err(e)) => (None, None, Some(e.to_string())),
        None => (
            None,
            None,
            Some("the home directory could not be found".to_string()),
        ),
    };
    emit(
        &report,
        id.as_deref(),
        path.as_deref().map(|p| anon::shape(p, root.as_deref())),
        why.as_deref(),
        json,
        &command,
    );
}

fn emit(
    report: &Report,
    id: Option<&str>,
    path: Option<String>,
    why: Option<&str>,
    json: bool,
    command: &str,
) {
    let headline = format!(
        "ciginstaller hit a bug and stopped: {}",
        super::truncate(&report.panic_message, 120)
    );
    let saved = match (id, why) {
        (Some(id), _) => format!("a report is saved ({id}); nothing was sent"),
        (None, Some(w)) => format!("the report could not be saved ({w})"),
        (None, None) => "the report could not be saved".to_string(),
    };
    if json {
        let mut e = serde_json::json!({
            "code": "E1890", "kind": "internal", "message": headline,
            "hint": id.map_or_else(|| "ciginstaller doctor".to_string(), |i| format!("ciginstaller crash show {i}")),
            "subject": report.fingerprint,
        });
        if let Some(p) = &path {
            e["report"] = serde_json::json!(p);
        }
        let mut v = serde_json::json!({ "ok": false, "error": e });
        if !command.is_empty() {
            v["command"] = serde_json::json!(command);
        }
        println!("{v}");
        return;
    }
    eprintln!("error[E1890 internal]: {headline}");
    eprintln!("  {saved}");
    if let Some(i) = id {
        eprintln!("  = hint: ciginstaller crash show {i}   (exactly what sending it would carry)");
    }
    eprintln!("  = explain: ciginstaller explain E1890");
}

/// A source path as the report carries it: inside this tree, from `crates/` or `src/`; a
/// toolchain or dependency path by its own tail; never a directory of the person's.
pub fn crate_relative(file: &str) -> String {
    let p = file.replace('\\', "/");
    if let Some(i) = p.find("crates/") {
        return p[i..].to_string();
    }
    if p.starts_with("src/") || p.starts_with("xtask/") {
        return p;
    }
    if let Some(i) = p.find("/rustc/") {
        let tail = &p[i + "/rustc/".len()..];
        return format!("rustc/{}", tail.split_once('/').map_or(tail, |(_, t)| t));
    }
    if let Some(i) = p.rfind("/src/") {
        return p[i + 1..].to_string();
    }
    p.rsplit('/').next().unwrap_or(&p).to_string()
}

fn strip_hash(symbol: &str) -> &str {
    match symbol.rfind("::h") {
        Some(i)
            if symbol.len() - i == 19 && symbol[i + 3..].bytes().all(|b| b.is_ascii_hexdigit()) =>
        {
            &symbol[..i]
        }
        _ => symbol,
    }
}

/// The frames a report keeps: the installer's own code (its crates), never the hook itself,
/// at most forty, source paths made crate-relative.
pub fn filter_backtrace(bt: &str) -> Vec<String> {
    const OURS: [&str; 7] = [
        "ciginstaller::",
        "cigi_home::",
        "cigi_source::",
        "cigi_ui::",
        "cigi_shim::",
        "cigi_sys::",
        "cig_compat::",
    ];
    let lines: Vec<&str> = bt.lines().collect();
    let mut out = Vec::new();
    for (i, raw) in lines.iter().enumerate() {
        let Some((n, symbol)) = raw.trim().split_once(": ") else {
            continue;
        };
        if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let bare = symbol.trim_start_matches('<');
        if !OURS.iter().any(|o| bare.starts_with(o)) || bare.contains("::crash::hook::") {
            continue;
        }
        let at = lines
            .get(i + 1)
            .map(|l| l.trim())
            .and_then(|l| l.strip_prefix("at "))
            .map(crate_relative);
        let symbol = strip_hash(symbol);
        out.push(match at {
            Some(a) => format!("{symbol} ({a})"),
            None => symbol.to_string(),
        });
        if out.len() == 40 {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_source_path_is_its_tail_in_this_tree() {
        assert_eq!(
            crate_relative("/home/jay/cig-installer/crates/ciginstaller/src/cmd/x.rs"),
            "crates/ciginstaller/src/cmd/x.rs"
        );
        assert_eq!(
            crate_relative("C:\\Users\\a\\w\\crates\\cigi-home\\src\\y.rs"),
            "crates/cigi-home/src/y.rs"
        );
        assert_eq!(
            crate_relative("/rustc/abc123/library/core/src/option.rs"),
            "rustc/library/core/src/option.rs"
        );
        assert_eq!(
            crate_relative("/home/jay/.cargo/registry/src/index-1/ureq-2.9.1/src/a.rs"),
            "src/a.rs"
        );
        assert_eq!(crate_relative("/somewhere/else.rs"), "else.rs");
    }

    #[test]
    fn only_the_installers_own_frames_stay() {
        let bt = "   0: std::backtrace::Backtrace::force_capture\n   1: ciginstaller::crash::hook::install::{{closure}}\n   2: core::panicking::panic_fmt\n             at /rustc/9f/library/core/src/panicking.rs:72:14\n   3: ciginstaller::cmd::crash::run::h0123456789abcdef\n             at /home/jay/w/crates/ciginstaller/src/cmd/crash.rs:30:9\n   4: main\n";
        assert_eq!(
            filter_backtrace(bt),
            vec![
                "ciginstaller::cmd::crash::run (crates/ciginstaller/src/cmd/crash.rs:30:9)"
                    .to_string()
            ]
        );
    }
}
