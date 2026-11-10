// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! `Ui`: modes, detection from the environment and flags, and where output goes.
//!
//! Streams: the result of a command (`say`, `table`) goes to stdout; chatter (running steps,
//! progress, prompts, notices) and errors go to stderr, so `which --path` and `--json` stay
//! clean when piped. Everything is rendered by pure functions first; printing is the thin
//! last step, and tests capture it with `Ui::captured`.

use crate::catalogue::Msg;
use crate::detect::{self, Rgb};
use crate::paint::{paint, ColorMode};
use crate::tokens::{budget, Role, Theme};
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

/// The `voice` config key: the family's voice, or plain words always.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VoiceChoice {
    #[default]
    Voice,
    Plain,
}

impl VoiceChoice {
    /// The `voice` config key: `voice` or `plain`.
    pub fn parse(s: &str) -> Option<VoiceChoice> {
        match s {
            "voice" => Some(VoiceChoice::Voice),
            "plain" => Some(VoiceChoice::Plain),
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
    /// `voice = plain` in config: plain words, permanently.
    pub voice: VoiceChoice,
    /// `--wide`: tables are never truncated.
    pub wide: bool,
    /// Never ask the terminal anything (no background query): the shim's slow path.
    pub no_probe: bool,
}

/// What the process looks like from outside: variables, terminals, width. `Env::real()` reads
/// the process; tests build one by hand.
#[derive(Debug, Clone, Default)]
pub struct Env {
    pub vars: BTreeMap<String, String>,
    pub tty_out: bool,
    pub tty_err: bool,
    pub tty_in: bool,
    /// The terminal's background colour, when it has been asked (OSC 11). Tests set it by hand.
    pub background: Option<Rgb>,
}

impl Env {
    pub fn real() -> Env {
        Env {
            vars: std::env::vars().collect(),
            tty_out: std::io::stdout().is_terminal(),
            tty_err: std::io::stderr().is_terminal(),
            tty_in: std::io::stdin().is_terminal(),
            background: None,
        }
    }

    pub fn with(mut self, k: &str, v: &str) -> Env {
        self.vars.insert(k.to_string(), v.to_string());
        self
    }

    /// Is a variable set to something other than empty, `0` or `false`?
    fn on(&self, k: &str) -> bool {
        self.get(k).is_some_and(|v| !matches!(v, "0" | "false"))
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
    /// Was the theme found (setting, `COLORFGBG` or the background query)? When not, the theme
    /// is `Neutral`: text stays at the terminal's own foreground and only glyphs are colored.
    pub theme_known: bool,
    pub width: usize,
    /// `--wide`: tables are never truncated.
    pub wide: bool,
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
            theme_known: true,
            width: 80,
            wide: false,
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

/// Is a locale name English? `C`, `POSIX`, an empty name and `en`, `en_AU.UTF-8` and the like are.
fn english(lang: &str) -> bool {
    let code = lang
        .split(['_', '.', '@', '-'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(code.as_str(), "" | "c" | "posix" | "en")
}

/// ASCII icons (`[ok] [..] [?] [x] [!] [-]`) when the locale is not UTF-8 (`LANG=C`), the
/// terminal is dumb, or the terminal is one whose font is known to lack the glyphs (the
/// Linux console and the VT100 family: the font check). `CIG_ASCII=1` forces ASCII and
/// `CIG_ASCII=0` forces the Unicode icons.
fn ascii_wanted(env: &Env, utf8: bool, term_dumb: bool) -> bool {
    match env.get("CIG_ASCII") {
        Some("0" | "false") => return false,
        Some(_) => return true,
        None => {}
    }
    let bare_font = matches!(
        env.get("TERM"),
        Some("linux" | "vt100" | "vt102" | "vt220" | "ansi")
    );
    !utf8 || term_dumb || bare_font
}

impl Ui {
    /// Detect from the real process: `NO_COLOR`, `--no-color`, `TERM=dumb`, `COLORTERM`,
    /// `LANG=C`, `--plain`, `--json`, `CIG_PLAIN`, `COLUMNS`, `COLORFGBG`, `theme`.
    pub fn from_env_and_flags(flags: &GlobalFlags) -> Ui {
        let mut env = Env::real();
        if Ui::would_probe(flags, &env, detect::in_foreground()) {
            let within = std::time::Duration::from_millis(budget::BACKGROUND_QUERY_MS);
            env.background = detect::query_background(within);
        }
        Ui::from_env(flags, &env)
    }

    /// Should the terminal be asked for its background (OSC 11)? `foreground` is whether this
    /// process is in the terminal's foreground group (`detect::in_foreground`). Only when the theme is on
    /// `auto`, `COLORFGBG` has not said, a person is at a terminal that will paint color, the
    /// output is in the voice (not plain, `--json`, CI, a screen reader or a non-English
    /// locale), the session is not over SSH (a slow link would deliver the reply late), and this
    /// process is in the terminal's foreground group. The query itself is bounded (see
    /// `detect::query_background`).
    pub fn would_probe(flags: &GlobalFlags, env: &Env, foreground: bool) -> bool {
        let term = env.get("TERM").unwrap_or("");
        flags.theme == ThemeChoice::Auto
            && !flags.no_probe
            && !flags.no_color
            && env.tty_in
            && env.tty_out
            && env.get("COLORFGBG").is_none()
            && env.get("NO_COLOR").is_none()
            && env.get("SSH_CONNECTION").is_none()
            && env.get("SSH_TTY").is_none()
            && !matches!(term, "" | "dumb" | "linux")
            && Ui::from_env(flags, env).mode == Mode::Voice
            && foreground
    }

    pub fn from_env(flags: &GlobalFlags, env: &Env) -> Ui {
        // Plain is automatic under CI and under a screen reader (#31), and permanent with
        // `voice = plain`; `--json` is structured and wins over all of them.
        // A terminal cannot say whether a screen reader is attached, and the desktop variables
        // that look like it (`QT_ACCESSIBILITY=1`) are set by default on KDE sessions, so they
        // are not read: that would make everyone's output plain. The signal is `CIG_SCREEN_READER`.
        let screen_reader = env.on("CIG_SCREEN_READER");
        // The voice is English-only (#31): other languages get plain until a voice is written.
        let lang = ["LC_ALL", "LC_MESSAGES", "LANG"]
            .iter()
            .find_map(|k| env.get(k))
            .unwrap_or("C");
        let plain_env = env.on("CIG_PLAIN") || env.on("CI") || screen_reader || !english(lang);
        let mode = if flags.json {
            Mode::Json
        } else if flags.plain || plain_env || flags.voice == VoiceChoice::Plain {
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
        let (theme, theme_known) = match flags.theme {
            ThemeChoice::Dark => (Theme::Dark, true),
            ThemeChoice::Light => (Theme::Light, true),
            ThemeChoice::Auto => match env
                .get("COLORFGBG")
                .and_then(detect::parse_colorfgbg)
                .or_else(|| env.background.map(detect::theme_of_background))
            {
                Some(t) => (t, true),
                None => (Theme::Neutral, false),
            },
        };
        let width = env
            .get("COLUMNS")
            .and_then(|c| c.parse::<usize>().ok())
            .map_or(80, |c| c.clamp(40, 200));
        Ui {
            mode,
            color: on(env.tty_out),
            ascii: ascii_wanted(env, utf8, term_dumb),
            tty_out: env.tty_out,
            tty_in: env.tty_in,
            tty_err: env.tty_err,
            color_err: on(env.tty_err),
            theme,
            theme_known,
            width,
            wide: flags.wide,
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

    /// A copy with a fresh notice slot (progress bars carry one, and the command's flows).
    pub fn duplicate(&self) -> Ui {
        Ui {
            mode: self.mode,
            color: self.color,
            ascii: self.ascii,
            tty_out: self.tty_out,
            tty_in: self.tty_in,
            tty_err: self.tty_err,
            color_err: self.color_err,
            theme: self.theme,
            theme_known: self.theme_known,
            width: self.width,
            wide: self.wide,
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

    /// Color for text. When the theme is not known the text stays at the terminal's own
    /// foreground (only bold is applied): no color is safe for words on an unknown background.
    pub(crate) fn paint(&self, s: Stream, role: Role, text: &str) -> String {
        if self.theme == Theme::Neutral && role != Role::Bold {
            return text.to_string();
        }
        paint(self.color_for(s), self.theme, role, text)
    }

    /// Color for a non-text glyph (an icon, an arrow, a bar). Uses the neutral palette, every
    /// color of which is at least 3:1 on both black and white, when the theme is not known.
    pub(crate) fn paint_glyph(&self, s: Stream, role: Role, text: &str) -> String {
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

    /// Real keyboard and screen (not a capture)?
    pub(crate) fn is_std(&self) -> bool {
        matches!(self.io, Io::Std)
    }

    /// Is the output in plain words (`--plain`, CI, a screen reader, `voice = plain`, a non-English
    /// locale, or `--json`)? Everything that picks a form (the explain pages, the catalogue)
    /// asks this, never a flag alone.
    pub fn plain(&self) -> bool {
        self.mode != Mode::Voice
    }

    /// A catalogue message in the active mode (`--json` has no sentences; it gets the plain form).
    pub fn msg(&self, m: &Msg, args: &[&str]) -> String {
        m.render(self.mode != Mode::Voice, args)
    }

    /// `true` the first time, `false` after: at most one notice per command.
    pub(crate) fn take_notice_slot(&self) -> bool {
        !self.noticed.swap(true, Ordering::SeqCst)
    }

    /// Text with the glyphs the terminal cannot show swapped for ASCII.
    /// Every piece of text on its way to the terminal passes here: control characters are made
    /// visible first (see [`clean`]), then the glyphs become ASCII when the terminal needs it.
    pub(crate) fn glyphs(&self, text: &str) -> String {
        self.literal(&product_names(text))
    }

    /// Like `glyphs`, but a command or a path: product ids stay as typed.
    pub(crate) fn literal(&self, text: &str) -> String {
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
            .replace('─', "-")
    }

    pub(crate) fn arrow(&self) -> &'static str {
        if self.ascii {
            "->"
        } else {
            "→"
        }
    }
}

/// Product ids written as the products are named in prose: `cigcompile` is CigCompile and
/// `cigar` (or `cigarscript`) is CigarScript. Only a standalone word is renamed: never inside a
/// path, a flag, a file name or backticks, and `cigar` only where a product is meant (before a
/// version, a separator or the end of the text), so the command `cigar check` stays typeable.
pub fn product_names(text: &str) -> std::borrow::Cow<'_, str> {
    const PRODUCTS: [(&str, &str); 3] = [
        ("cigcompile", "CigCompile"),
        ("cigarscript", "CigarScript"),
        ("cigar", "CigarScript"),
    ];
    let glue = |c: char| c.is_alphanumeric() || "/-._`~$@=:\\".contains(c);
    let after_glue = |c: char| c.is_alphanumeric() || "/-._`=".contains(c);
    if !PRODUCTS.iter().any(|(id, _)| text.contains(id)) {
        return std::borrow::Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len() + 8);
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        let before = text[..i].chars().next_back();
        let hit = PRODUCTS.iter().find(|(id, _)| rest.starts_with(id));
        match hit {
            Some((id, name)) if !before.is_some_and(glue) => {
                let tail = &rest[id.len()..];
                let next = tail.chars().next();
                let product_use = *id != "cigar"
                    || match next {
                        None => true,
                        Some(c) if ",;:)".contains(c) => true,
                        Some(' ') => tail[1..].starts_with(|c: char| c.is_ascii_digit()),
                        _ => false,
                    };
                if !next.is_some_and(after_glue) && product_use {
                    out.push_str(name);
                    i += id.len();
                    continue;
                }
                out.push_str(&rest[..id.len()]);
                i += id.len();
            }
            _ => {
                let c = rest.chars().next().unwrap_or(' ');
                out.push(c);
                i += c.len_utf8();
            }
        }
    }
    std::borrow::Cow::Owned(out)
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
