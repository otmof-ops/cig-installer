// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! The output grammar (SPEC section 11.1, issue #32): every message is an icon and a
//! headline, optional detail lines indented 2, and optionally one next step.
//!
//! ```text
//! <icon> <headline>                 always; the code is right-aligned in voice mode
//!   <detail>                        0..n lines
//!   → <next>                        0..1 line (a finding may have one per fix)
//! ```

use crate::error::{InstallerError, EXIT_BUG, EXIT_OK, EXIT_REFUSED};
use crate::registry;
use crate::tokens::Role;
use crate::ui::{Mode, Stream, Ui};
use cig_compat::check::{Finding, Severity};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Done,
    Running,
    Ask,
    Refused,
    Bug,
    Note,
    Irreversible,
}

impl Icon {
    pub fn glyph(self, ascii: bool) -> &'static str {
        match (self, ascii) {
            (Icon::Done, false) => "✓",
            (Icon::Running, false) => "●",
            (Icon::Ask, false) => "?",
            (Icon::Refused | Icon::Bug, false) => "✗",
            (Icon::Note, false) => "·",
            (Icon::Irreversible, false) => "!",
            (Icon::Ask, true) => "[?]",
            (Icon::Irreversible, true) => "[!]",
            (Icon::Done, true) => "[ok]",
            (Icon::Running, true) => "[..]",
            (Icon::Refused | Icon::Bug, true) => "[x]",
            (Icon::Note, true) => "[-]",
        }
    }

    pub fn plain_prefix(self) -> &'static str {
        match self {
            Icon::Done => "ok:",
            Icon::Running => "...",
            Icon::Ask => "question:",
            Icon::Refused => "refused:",
            Icon::Bug => "bug:",
            Icon::Note => "note:",
            Icon::Irreversible => "irreversible:",
        }
    }

    pub fn role(self) -> Role {
        match self {
            Icon::Done => Role::Ok,
            Icon::Running => Role::EmberBright,
            Icon::Ask => Role::Warn,
            Icon::Bug | Icon::Irreversible => Role::Danger,
            // A refusal is painted by its kind of no (see `Ui::line`); without one it is a
            // plain "no" and reads as a warning, never as a bug.
            Icon::Refused => Role::Warn,
            Icon::Note => Role::Ash400,
        }
    }

    /// Did this go well?
    pub fn is_ok(self) -> bool {
        !matches!(self, Icon::Refused | Icon::Bug)
    }
}

#[derive(Debug, Clone)]
pub struct Report {
    pub icon: Icon,
    pub headline: String,
    pub detail: Vec<String>,
    pub next: Option<String>,
    pub code: Option<&'static str>,
    pub data: serde_json::Value,
}

impl Report {
    pub fn new(icon: Icon, headline: impl Into<String>) -> Report {
        Report {
            icon,
            headline: headline.into(),
            detail: Vec::new(),
            next: None,
            code: None,
            data: serde_json::Value::Null,
        }
    }
    pub fn done(headline: impl Into<String>) -> Report {
        Report::new(Icon::Done, headline)
    }
    pub fn detail(mut self, line: impl Into<String>) -> Report {
        self.detail.push(line.into());
        self
    }
    pub fn next(mut self, cmd: impl Into<String>) -> Report {
        self.next = Some(cmd.into());
        self
    }
    pub fn code(mut self, code: &'static str) -> Report {
        self.code = Some(code);
        self
    }
    pub fn data(mut self, data: serde_json::Value) -> Report {
        self.data = data;
        self
    }
}

/// One finding of `cig-compat::check`, as a report: a refusal is `✗` with its code, a
/// warning or note is `·`. Only the first fix fits `Report::next`; use `Ui::finding` to
/// print every fix, one `→` line each.
pub fn finding_report(f: &Finding) -> Report {
    let icon = if f.severity == Severity::Refuse {
        Icon::Refused
    } else {
        Icon::Note
    };
    Report {
        icon,
        headline: f.headline.clone(),
        detail: f.detail.clone(),
        next: f.fixes.first().cloned(),
        code: Some(f.code),
        data: serde_json::Value::Null,
    }
}

#[derive(Serialize)]
struct JsonReport<'a> {
    ok: bool,
    command: &'a str,
    headline: &'a str,
    detail: &'a [String],
    next: Option<&'a str>,
    code: Option<&'a str>,
    data: &'a serde_json::Value,
}

#[derive(Serialize)]
struct JsonErrorBody<'a> {
    code: &'a str,
    kind: &'a str,
    message: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    subject: Option<&'a str>,
}

#[derive(Serialize)]
struct JsonError<'a> {
    ok: bool,
    #[serde(skip_serializing_if = "str::is_empty")]
    command: &'a str,
    error: JsonErrorBody<'a>,
}

impl Ui {
    fn line(&self, s: Stream, icon: Icon, role: Role, text: &str) -> (String, usize) {
        // returns the painted line and its visible width
        let text = self.glyphs(text);
        match self.mode {
            Mode::Plain => {
                let pre = icon.plain_prefix();
                (
                    format!("{} {text}", self.paint(s, role, pre)),
                    pre.chars().count() + 1 + text.chars().count(),
                )
            }
            _ => {
                let g = icon.glyph(self.ascii);
                (
                    format!("{} {text}", self.paint(s, role, g)),
                    g.chars().count() + 1 + text.chars().count(),
                )
            }
        }
    }

    fn grammar(
        &self,
        s: Stream,
        icon: Icon,
        headline: &str,
        detail: &[String],
        nexts: &[String],
        code: Option<&str>,
    ) -> String {
        let role = match (icon, code) {
            (Icon::Refused, Some(c)) => registry::role_of(c),
            _ => icon.role(),
        };
        let headline = match (icon, code) {
            (Icon::Refused, Some(c)) => match self.kind_label(c) {
                Some(label) => format!("{label}: {headline}"),
                None => headline.to_string(),
            },
            _ => headline.to_string(),
        };
        let (mut first, width) = self.line(s, icon, role, &headline);
        if let Some(c) = code {
            match self.mode {
                Mode::Plain => first.push_str(&format!(" ({c})")),
                _ => {
                    let pad = self.width.saturating_sub(width + c.len()).max(2);
                    first.push_str(&" ".repeat(pad));
                    first.push_str(&self.paint(s, Role::Ash400, c));
                }
            }
        }
        let mut out = first + "\n";
        for d in detail {
            out.push_str(&format!("  {}\n", self.glyphs(d)));
        }
        for n in nexts {
            let n = self.literal(n);
            if self.mode == Mode::Plain {
                out.push_str(&format!("  next: {n}\n"));
            } else {
                out.push_str(&format!(
                    "  {} {}\n",
                    self.paint(s, Role::Ember, self.arrow()),
                    n
                ));
            }
        }
        out
    }

    /// The text `say` would print for a report (not for Json mode).
    pub fn render_report(&self, r: &Report) -> String {
        let s = if r.icon == Icon::Running {
            Stream::Err
        } else {
            Stream::Out
        };
        let nexts: Vec<String> = r.next.iter().cloned().collect();
        self.grammar(s, r.icon, &r.headline, &r.detail, &nexts, r.code)
    }

    /// Print a report. Results go to stdout, running steps to stderr; `--quiet` drops running
    /// steps and notes; in Json mode nothing is printed here (see `finish`).
    pub fn say(&self, r: &Report) {
        if self.mode == Mode::Json || (self.quiet && matches!(r.icon, Icon::Running | Icon::Note)) {
            return;
        }
        let s = if r.icon == Icon::Running {
            Stream::Err
        } else {
            Stream::Out
        };
        self.write(s, &self.render_report(r));
    }

    /// A `check` finding: the headline with its code, the detail, and every fix as a `→` line.
    pub fn render_finding(&self, f: &Finding) -> String {
        let icon = if f.severity == Severity::Refuse {
            Icon::Refused
        } else {
            Icon::Note
        };
        self.grammar(
            Stream::Out,
            icon,
            &f.headline,
            &f.detail,
            &f.fixes,
            Some(f.code),
        )
    }

    pub fn finding(&self, f: &Finding) {
        if self.mode != Mode::Json {
            self.write(Stream::Out, &self.render_finding(f));
        }
    }

    /// The kind of no a code is, in the active mode: the voice's catchphrase ("can't see any
    /// ciggies bro") or plain's name for it ("not found"). `None` for a code that has no kind.
    fn kind_label(&self, code: &str) -> Option<&'static str> {
        match self.mode {
            Mode::Voice => registry::catchphrase(code),
            _ => registry::plain_label(code),
        }
    }

    /// `error[E1811 carton]: <message>` with subject, hint, explain, doctor and fix lines.
    /// The code and its area are always shown; the colour follows the kind of no (#31).
    pub fn render_error(&self, e: &InstallerError) -> String {
        let internal = e.kind == crate::error::Kind::Internal;
        let icon = if internal { Icon::Bug } else { Icon::Refused };
        let role = if internal {
            Role::Danger
        } else {
            registry::role_of(e.code)
        };
        let said = match self.kind_label(e.code) {
            Some(label) => format!("{label}: {}", e.message),
            None => e.message.clone(),
        };
        let head = format!("error[{} {}]: {said}", e.code, registry::area(e.code));
        let (first, _) = self.line(Stream::Err, icon, role, &head);
        let mut out = first + "\n";
        let mut eq = |label: &str, v: String| {
            out.push_str(&format!(
                "  {} {v}\n",
                self.paint(Stream::Err, Role::Ash400, &format!("= {label}:")),
            ))
        };
        if let Some(s) = &e.subject {
            eq("subject", self.glyphs(s));
        }
        if let Some(h) = &e.hint {
            eq("hint", self.glyphs(h));
        }
        eq(
            "explain",
            self.literal(&format!("ciginstaller explain {}", e.code)),
        );
        if let Some(d) = &e.doctor {
            eq("doctor", self.glyphs(d));
        }
        if let Some(f) = e.fix_text() {
            eq("fix", self.literal(&f));
        }
        out
    }

    pub fn error(&self, e: &InstallerError) {
        self.error_for("", e);
    }

    /// Like `error`, naming the command in the JSON object.
    pub fn error_for(&self, command: &str, e: &InstallerError) {
        if self.mode == Mode::Json {
            let body = JsonErrorBody {
                code: e.code,
                kind: e.kind.as_str(),
                message: &e.message,
                hint: e.fix_text_for_json(),
                subject: e.subject.as_deref(),
            };
            let obj = JsonError {
                ok: false,
                command,
                error: body,
            };
            self.write(
                Stream::Out,
                &format!("{}\n", serde_json::to_string(&obj).unwrap_or_default()),
            );
        } else {
            self.write(Stream::Err, &self.render_error(e));
        }
    }

    /// Print a command's outcome in the active mode and return its exit code: `0` for a
    /// report (`2` for a refused one, `70` for a bug), the error's `exit` for an error.
    pub fn finish(&self, command: &str, result: &Result<Report, InstallerError>) -> i32 {
        match result {
            Ok(r) => {
                if self.mode == Mode::Json {
                    let obj = JsonReport {
                        ok: r.icon.is_ok(),
                        command,
                        headline: &r.headline,
                        detail: &r.detail,
                        next: r.next.as_deref(),
                        code: r.code,
                        data: &r.data,
                    };
                    self.write(
                        Stream::Out,
                        &format!("{}\n", serde_json::to_string(&obj).unwrap_or_default()),
                    );
                } else {
                    self.say(r);
                }
                match r.icon {
                    Icon::Refused => EXIT_REFUSED,
                    Icon::Bug => EXIT_BUG,
                    _ => EXIT_OK,
                }
            }
            Err(e) => {
                self.error_for(command, e);
                e.exit
            }
        }
    }

    /// A page of text, such as an `explain` page, as the result on stdout. In Json mode it is
    /// one object, `{"command": .., "page": ..}`.
    pub fn page(&self, command: &str, text: &str) {
        if self.mode == Mode::Json {
            let obj = serde_json::json!({ "ok": true, "command": command, "page": text });
            self.write(Stream::Out, &format!("{obj}\n"));
        } else {
            let mut t = text.to_string();
            if !t.ends_with('\n') {
                t.push('\n');
            }
            self.write(Stream::Out, &t);
        }
    }

    /// At most one line at the end of a command; later calls are ignored. Never in Json mode.
    pub fn notice(&self, line: &str) {
        if self.mode == Mode::Json || self.quiet || !self.take_notice_slot() {
            return;
        }
        let (l, _) = self.line(Stream::Err, Icon::Note, Icon::Note.role(), line);
        self.write(Stream::Err, &format!("{l}\n"));
    }
}

impl InstallerError {
    /// cig's `hint` is "what to type next": the flow's fix, else the registry's, else the plain hint.
    fn fix_text_for_json(&self) -> Option<String> {
        self.fix_text()
            .or_else(|| self.hint.as_deref().map(str::to_string))
    }
}
