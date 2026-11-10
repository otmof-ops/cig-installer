// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! `Ui`: modes, detection from the environment and flags, and where output goes.
//!
//! Streams: the result of a command (`say`, `table`) goes to stdout; chatter (running steps,
//! progress, prompts, notices) and errors go to stderr, so `which --path` and `--json` stay
//! clean when piped. Everything is rendered by pure functions first; printing is the thin
//! last step, and tests capture it with `Ui::captured`.

use crate::tokens::{paint, ColorMode, Role, Theme};
use std::collections::{BTreeMap, VecDeque};
use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Voice,
    Plain,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeChoice {
    #[default]
    Auto,
    Dark,
    Light,
}

impl ThemeChoice {
    /// The `theme` config key: `auto`, `dark` or `light`.
    pub fn parse(s: &str) -> Option<ThemeChoice> {
        match s {
            "auto" => Some(ThemeChoice::Auto),
            "dark" => Some(ThemeChoice::Dark),
            "light" => Some(ThemeChoice::Light),
            _ => None,
        }
    }
}

/// The global flags of SPEC section 10 that the UI cares about.
#[derive(Debug, Clone, Default)]
pub struct GlobalFlags {
    pub json: bool,
    pub plain: bool,
    pub no_color: bool,
    pub yes: bool,
    pub yes_irreversible: bool,
    pub quiet: bool,
    pub verbose: bool,
    pub offline: bool,
    pub home: Option<PathBuf>,
    pub theme: ThemeChoice,
}

/// What the process looks like from outside: variables, terminals, width. `Env::real()` reads
/// the process; tests build one by hand.
#[derive(Debug, Clone, Default)]
pub struct Env {
    pub vars: BTreeMap<String, String>,
    pub tty_out: bool,
    pub tty_err: bool,
    pub tty_in: bool,
}

impl Env {
    pub fn real() -> Env {
        Env {
            vars: std::env::vars().collect(),
            tty_out: std::io::stdout().is_terminal(),
            tty_err: std::io::stderr().is_terminal(),
            tty_in: std::io::stdin().is_terminal(),
        }
    }

    pub fn with(mut self, k: &str, v: &str) -> Env {
        self.vars.insert(k.to_string(), v.to_string());
        self
    }

    fn get(&self, k: &str) -> Option<&str> {
        self.vars
            .get(k)
            .map(String::as_str)
            .filter(|v| !v.is_empty())
    }
}

#[derive(Debug)]
pub struct Ui {
    pub mode: Mode,
    /// Colour depth for stdout.
    pub color: ColorMode,
    pub ascii: bool,
    pub tty_out: bool,
    pub tty_in: bool,
    // Additions to the SPEC 2.4 struct (see the report): the pieces #32 needs.
    pub tty_err: bool,
    /// Colour depth for stderr (stdout may be piped while the terminal is not).
    pub color_err: ColorMode,
    pub theme: Theme,
    pub width: usize,
    pub term_dumb: bool,
    pub quiet: bool,
    pub verbose: bool,
    pub yes: bool,
    pub yes_irreversible: bool,
    noticed: AtomicBool,
    io: Io,
    /// Everything goes to stderr: the shim's slow path, where stdout is the tool's.
    err_only: bool,
}

impl Default for Ui {
    /// Voice, no colour, Unicode, no terminals, 80 columns.
    fn default() -> Ui {
        Ui {
            mode: Mode::Voice,
            color: ColorMode::None,
            ascii: false,
            tty_out: false,
            tty_in: false,
            tty_err: false,
            color_err: ColorMode::None,
            theme: Theme::Dark,
            width: 80,
            term_dumb: false,
            quiet: false,
            verbose: false,
            yes: false,
            yes_irreversible: false,
            noticed: AtomicBool::new(false),
            io: Io::Std,
            err_only: false,
        }
    }
}

#[derive(Debug, Clone)]
enum Io {
    Std,
    Captured(Arc<Mutex<Captured>>),
}

/// What a captured `Ui` wrote, and what it will read as keyboard input.
#[derive(Debug, Default)]
pub struct Captured {
    pub out: String,
    pub err: String,
    pub input: VecDeque<String>,
}

/// A handle to read what a captured `Ui` printed.
#[derive(Debug, Clone)]
pub struct Capture(Arc<Mutex<Captured>>);

impl Capture {
    pub fn out(&self) -> String {
        self.0.lock().map(|c| c.out.clone()).unwrap_or_default()
    }
    pub fn err(&self) -> String {
        self.0.lock().map(|c| c.err.clone()).unwrap_or_default()
    }
    /// Queue a line the next prompt will read.
    pub fn push_input(&self, line: &str) {
        if let Ok(mut c) = self.0.lock() {
            c.input.push_back(line.to_string());
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stream {
    Out,
    Err,
}

impl Ui {
    /// Detect from the real process: `NO_COLOR`, `--no-color`, `TERM=dumb`, `COLORTERM`,
    /// `LANG=C`, `--plain`, `--json`, `CIG_PLAIN`, `COLUMNS`, `COLORFGBG`, `theme`.
    pub fn from_env_and_flags(flags: &GlobalFlags) -> Ui {
        Ui::from_env(flags, &Env::real())
    }

    pub fn from_env(flags: &GlobalFlags, env: &Env) -> Ui {
        let plain_env = env.get("CIG_PLAIN").is_some_and(|v| v != "0");
        let mode = if flags.json {
            Mode::Json
        } else if flags.plain || plain_env {
            Mode::Plain
        } else {
            Mode::Voice
        };
        let term_dumb = env.get("TERM") == Some("dumb");
        let no_color =
            flags.no_color || env.get("NO_COLOR").is_some() || term_dumb || mode == Mode::Json;
        let depth = match (env.get("COLORTERM"), env.get("TERM")) {
            (Some("truecolor" | "24bit"), _) => ColorMode::True,
            (_, Some(t)) if t.contains("256color") => ColorMode::Ansi256,
            _ => ColorMode::Ansi16,
        };
        let on = |tty: bool| {
            if tty && !no_color {
                depth
            } else {
                ColorMode::None
            }
        };
        let locale = ["LC_ALL", "LC_CTYPE", "LANG"]
            .iter()
            .find_map(|k| env.get(k))
            .unwrap_or("C")
            .to_ascii_lowercase();
        let utf8 = locale.contains("utf-8") || locale.contains("utf8");
        let theme = match flags.theme {
            ThemeChoice::Dark => Theme::Dark,
            ThemeChoice::Light => Theme::Light,
            ThemeChoice::Auto => match env
                .get("COLORFGBG")
                .and_then(|v| v.rsplit(';').next())
                .and_then(|b| b.parse::<u8>().ok())
            {
                Some(bg) if bg == 7 || (9..=15).contains(&bg) => Theme::Light,
                _ => Theme::Dark,
            },
        };
        let width = env
            .get("COLUMNS")
            .and_then(|c| c.parse::<usize>().ok())
            .map_or(80, |c| c.clamp(40, 200));
        Ui {
            mode,
            color: on(env.tty_out),
            ascii: !utf8 || term_dumb,
            tty_out: env.tty_out,
            tty_in: env.tty_in,
            tty_err: env.tty_err,
            color_err: on(env.tty_err),
            theme,
            width,
            term_dumb,
            quiet: flags.quiet,
            verbose: flags.verbose,
            yes: flags.yes,
            yes_irreversible: flags.yes_irreversible,
            noticed: AtomicBool::new(false),
            io: Io::Std,
            err_only: false,
        }
    }

    /// A copy with a fresh notice slot (progress bars carry one).
    pub(crate) fn duplicate(&self) -> Ui {
        Ui {
            mode: self.mode,
            color: self.color,
            ascii: self.ascii,
            tty_out: self.tty_out,
            tty_in: self.tty_in,
            tty_err: self.tty_err,
            color_err: self.color_err,
            theme: self.theme,
            width: self.width,
            term_dumb: self.term_dumb,
            quiet: self.quiet,
            verbose: self.verbose,
            yes: self.yes,
            yes_irreversible: self.yes_irreversible,
            noticed: AtomicBool::new(self.noticed.load(Ordering::SeqCst)),
            io: self.io.clone(),
            err_only: self.err_only,
        }
    }

    /// Send everything to stderr, results included, painted for stderr: the shim's slow path,
    /// where stdout belongs to the tool it starts and a note there would reach the tool's reader.
    pub fn all_to_stderr(mut self) -> Ui {
        self.err_only = true;
        self.color = self.color_err;
        self.tty_out = self.tty_err;
        self
    }

    /// Attach a capture: everything this `Ui` prints is kept instead of written.
    pub fn captured(mut self) -> (Ui, Capture) {
        let shared = Arc::new(Mutex::new(Captured::default()));
        self.io = Io::Captured(Arc::clone(&shared));
        (self, Capture(shared))
    }

    pub(crate) fn color_for(&self, s: Stream) -> ColorMode {
        match s {
            Stream::Out => self.color,
            Stream::Err => self.color_err,
        }
    }

    pub(crate) fn paint(&self, s: Stream, role: Role, text: &str) -> String {
        paint(self.color_for(s), self.theme, role, text)
    }

    /// Write already-rendered text (it carries its own newlines).
    pub(crate) fn write(&self, s: Stream, text: &str) {
        let s = if self.err_only { Stream::Err } else { s };
        match &self.io {
            Io::Std => {
                // A closed pipe is the reader's choice; never fail a command over it.
                let _ = match s {
                    Stream::Out => std::io::stdout()
                        .write_all(text.as_bytes())
                        .and_then(|()| std::io::stdout().flush()),
                    Stream::Err => std::io::stderr()
                        .write_all(text.as_bytes())
                        .and_then(|()| std::io::stderr().flush()),
                };
            }
            Io::Captured(c) => {
                if let Ok(mut c) = c.lock() {
                    match s {
                        Stream::Out => c.out.push_str(text),
                        Stream::Err => c.err.push_str(text),
                    }
                }
            }
        }
    }

    /// One line from the keyboard (or the captured queue). `None` at end of input.
    pub(crate) fn read_line(&self) -> Option<String> {
        match &self.io {
            Io::Std => {
                let mut line = String::new();
                match std::io::stdin().read_line(&mut line) {
                    Ok(0) | Err(_) => None,
                    Ok(_) => Some(line.trim_end_matches(['\r', '\n']).to_string()),
                }
            }
            Io::Captured(c) => c.lock().ok().and_then(|mut c| c.input.pop_front()),
        }
    }

    /// `true` the first time, `false` after: at most one notice per command.
    pub(crate) fn take_notice_slot(&self) -> bool {
        !self.noticed.swap(true, Ordering::SeqCst)
    }

    /// Text with the glyphs the terminal cannot show swapped for ASCII.
    /// Every piece of text on its way to the terminal passes here: control characters are made
    /// visible first (see [`clean`]), then the glyphs become ASCII when the terminal needs it.
    pub(crate) fn glyphs(&self, text: &str) -> String {
        let text = clean(text);
        if !self.ascii {
            return text.into_owned();
        }
        text.replace('→', "->")
            .replace('—', "-")
            .replace('…', "...")
            .replace('·', "-")
            .replace('━', "=")
            .replace('╸', ">")
    }

    pub(crate) fn arrow(&self) -> &'static str {
        if self.ascii {
            "->"
        } else {
            "→"
        }
    }
}

/// Text from outside the installer (a project's pin file, a manifest's notes, a path) can hold
/// escape sequences: a clear-screen, a new title, a clipboard write, a fake `[ok]` line. Every
/// control character but tab, and the bidirectional overrides that reorder what is shown, is
/// written out as its escape (`\u{1b}`), so the terminal shows it instead of obeying it. JSON
/// output needs none of this: serde escapes control characters itself.
pub fn clean(text: &str) -> std::borrow::Cow<'_, str> {
    fn unsafe_char(c: char) -> bool {
        (c.is_control() && c != '\t')
            || matches!(c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
    }
    if !text.chars().any(unsafe_char) {
        return std::borrow::Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        if unsafe_char(c) {
            out.extend(c.escape_unicode());
        } else {
            out.push(c);
        }
    }
    std::borrow::Cow::Owned(out)
}
