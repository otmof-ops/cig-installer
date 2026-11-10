// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! Prompts (#32): the default is the safe answer (for anything irreversible, no, said in the
//! prompt). Without a terminal nothing is asked: the safe default is taken, one line says
//! so, and the answer tells the caller to exit 3 when that default was a refusal.
//!
//! On a terminal one keypress answers, with no enter (see `detect::read_key`: terminal modes
//! are set and restored by a trapped `sh` script, so there is no `unsafe` and a Ctrl-C leaves
//! the terminal as it was). Off a terminal the answer is a line from the input. Raw mode is
//! never used anywhere but on a terminal. Passwords and OS authentication never go through
//! here: they go to the OS.

use crate::catalogue as cat;
use crate::detect::{self, Key};
use crate::error::EXIT_USAGE;
use crate::grammar::Icon;
use crate::tokens::Role;
use crate::ui::{Mode, Stream, Ui};

#[derive(Debug, Clone)]
pub struct Choice {
    /// The key that selects it; case-insensitive.
    pub key: char,
    pub label: String,
    /// Taking this is a refusal (a "no"); decides the exit code without a terminal.
    pub refusal: bool,
    /// `--yes` takes the first affirmative choice.
    pub affirmative: bool,
}

impl Choice {
    pub fn new(key: char, label: &str) -> Choice {
        Choice {
            key,
            label: label.to_string(),
            refusal: false,
            affirmative: false,
        }
    }
    pub fn yes() -> Choice {
        Choice {
            key: 'y',
            label: "yes".into(),
            refusal: false,
            affirmative: true,
        }
    }
    pub fn no() -> Choice {
        Choice {
            key: 'n',
            label: "no".into(),
            refusal: true,
            affirmative: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Question {
    pub text: String,
    /// Shown on the same line as the question.
    pub summary: Option<String>,
    pub detail: Vec<String>,
    pub choices: Vec<Choice>,
    /// Index of the safe default in `choices`.
    pub default: usize,
    pub irreversible: bool,
}

impl Question {
    /// Yes/no. The default is yes only when `default_yes` and the step is not irreversible.
    pub fn yes_no(text: &str, default_yes: bool) -> Question {
        Question {
            text: text.to_string(),
            summary: None,
            detail: Vec::new(),
            choices: vec![Choice::yes(), Choice::no()],
            default: if default_yes { 0 } else { 1 },
            irreversible: false,
        }
    }
    /// Yes/no about something that cannot be undone: the default is no, and `--yes` alone does not say yes.
    pub fn irreversible(text: &str) -> Question {
        Question {
            irreversible: true,
            ..Question::yes_no(text, false)
        }
    }
    pub fn summary(mut self, s: &str) -> Question {
        self.summary = Some(s.to_string());
        self
    }
    pub fn detail(mut self, line: &str) -> Question {
        self.detail.push(line.to_string());
        self
    }
    pub fn choice(mut self, c: Choice) -> Question {
        self.choices.push(c);
        self
    }
}

/// What one read from the keyboard came to.
enum Reply {
    Eof,
    Ignore,
    Enter,
    Text(String),
}

fn is_key(c: &Choice, typed: &str) -> bool {
    let mut chars = typed.chars();
    chars.next() == Some(c.key.to_ascii_lowercase()) && chars.next().is_none()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum How {
    /// The user typed a key.
    Typed,
    /// The user pressed enter on the default.
    Enter,
    /// `--yes` (or `--yes-irreversible`) answered.
    Flag,
    /// No terminal: the safe default was taken, and one line said so.
    NoTty,
    /// Three answers that were not options, or the input ended: nothing was taken, and the
    /// caller must stop without changing anything and exit 3.
    Aborted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Answer {
    pub index: usize,
    pub key: char,
    pub how: How,
    pub refusal: bool,
}

impl Answer {
    /// `Some(3)` when no question could be asked and the default was a refusal, or when the
    /// questions went unanswered (SPEC 11.2).
    pub fn exit_code(&self) -> Option<i32> {
        match self.how {
            How::Aborted => Some(EXIT_USAGE),
            How::NoTty if self.refusal => Some(EXIT_USAGE),
            _ => None,
        }
    }
    pub fn is_yes(&self, q: &Question) -> bool {
        self.how != How::Aborted && q.choices.get(self.index).is_some_and(|c| c.affirmative)
    }
}

impl Ui {
    fn pick(&self, q: &Question, index: usize, how: How) -> Answer {
        let i = index.min(q.choices.len().saturating_sub(1));
        let (key, refusal) = q
            .choices
            .get(i)
            .map_or(('n', true), |c| (c.key.to_ascii_lowercase(), c.refusal));
        Answer {
            index: i,
            key,
            how,
            refusal,
        }
    }

    fn choices_line(&self, q: &Question) -> String {
        if q.irreversible {
            let said = self.msg(&cat::TYPE_YES, &[]);
            return if self.mode == Mode::Plain {
                format!("  {said}")
            } else {
                said
            };
        }
        if self.mode == Mode::Plain {
            let parts: Vec<String> = q
                .choices
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    format!(
                        "{} ({}{})",
                        c.label,
                        c.key.to_ascii_lowercase(),
                        if i == q.default {
                            format!(", {}", self.msg(&cat::CHOICES_DEFAULT, &[]))
                        } else {
                            String::new()
                        }
                    )
                })
                .collect();
            return format!("  choices: {}", parts.join(", "));
        }
        let parts: Vec<String> = q
            .choices
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let key = if i == q.default {
                    c.key.to_ascii_uppercase()
                } else {
                    c.key.to_ascii_lowercase()
                };
                match c
                    .label
                    .strip_prefix(|l: char| l.eq_ignore_ascii_case(&c.key))
                {
                    Some(rest) => format!("[{key}]{rest}"),
                    None => format!("[{key}] {}", c.label),
                }
            })
            .collect();
        parts.join("  ")
    }

    /// The prompt as drawn, ending with a newline.
    pub fn render_prompt(&self, q: &Question) -> String {
        let s = Stream::Err;
        let head = match &q.summary {
            Some(sum) => format!("{}  {sum}", q.text),
            None => q.text.clone(),
        };
        // An irreversible question leads with `!` in `danger`, an ordinary one with `?` in `warn`.
        let (icon, role) = if q.irreversible {
            (Icon::Irreversible, Role::Danger)
        } else {
            (Icon::Ask, Role::Warn)
        };
        let lead = if self.mode == Mode::Plain {
            icon.plain_prefix()
        } else {
            icon.glyph(self.ascii)
        };
        let lead = if self.mode == Mode::Plain {
            self.paint(s, role, lead)
        } else {
            self.paint_glyph(s, role, lead)
        };
        let mut out = format!("{lead} {}\n", self.glyphs(&head));
        for d in &q.detail {
            out.push_str(&format!("  {}\n", self.glyphs(d)));
        }
        let default = q.choices.get(q.default).map_or("no", |c| c.label.as_str());
        let enter = if q.irreversible {
            self.msg(&cat::ENTER_DEFAULT_IRREVERSIBLE, &[default])
        } else {
            self.msg(&cat::ENTER_DEFAULT, &[default])
        };
        let line = self.choices_line(q);
        if self.mode == Mode::Plain {
            out.push_str(&format!("{line}\n  {enter}\n"));
        } else {
            let pad = self
                .width
                .saturating_sub(line.chars().count() + enter.chars().count())
                .max(2);
            out.push_str(&format!(
                "  {}{}{}\n",
                line.trim_start(),
                " ".repeat(pad.saturating_sub(2).max(2)),
                self.paint(s, Role::Ash400, &enter)
            ));
        }
        out
    }

    /// Ask, or take the safe default. See the module comment for the rules.
    pub fn ask(&self, q: &Question) -> Answer {
        let affirmative = q.choices.iter().position(|c| c.affirmative);
        let said_yes =
            (self.yes || self.yes_irreversible) && (!q.irreversible || self.yes_irreversible);
        if let (true, Some(i)) = (said_yes, affirmative) {
            self.chatter(&self.msg(&cat::FLAG_ANSWERED, &[&q.choices[i].label, &q.text]));
            return self.pick(q, i, How::Flag);
        }
        let can_ask = self.mode != Mode::Json && self.tty_in && self.tty_err;
        if !can_ask {
            let def = q.choices.get(q.default).map_or("no", |c| c.label.as_str());
            let why = if self.yes && q.irreversible {
                self.msg(&cat::WHY_IRREVERSIBLE, &[])
            } else {
                self.msg(&cat::WHY_PASS_YES, &[])
            };
            self.chatter(&self.msg(&cat::NO_TERMINAL, &[def, &q.text, &why]));
            return self.pick(q, q.default, How::NoTty);
        }
        self.write(Stream::Err, &self.render_prompt(q));
        // One keypress on a terminal, in the voice, for a question that can be undone. Under
        // plain (`--plain`, CI, a screen reader, `voice = plain`, a non-English locale) and for
        // anything irreversible the answer is a typed line.
        let mut keys = self.is_std() && !self.plain() && !q.irreversible;
        if !keys && self.is_std() {
            detect::drain_input();
        }
        let mut misses = 0;
        while misses < 3 {
            let reply = if keys {
                match detect::read_key() {
                    Some(Key::Char(c)) => Reply::Text(c.to_string()),
                    Some(Key::Enter) => Reply::Enter,
                    Some(Key::Eof) => Reply::Eof,
                    Some(Key::Ignored) => Reply::Ignore,
                    None => {
                        // no `sh`, `stty` or `dd`: fall back to a typed line
                        keys = false;
                        detect::drain_input();
                        continue;
                    }
                }
            } else {
                match self.read_line() {
                    None => Reply::Eof,
                    Some(l) if l.trim().is_empty() => Reply::Enter,
                    Some(l) => Reply::Text(l.trim().to_string()),
                }
            };
            match reply {
                Reply::Eof => break,
                Reply::Ignore => {}
                Reply::Enter => return self.pick(q, q.default, How::Enter),
                Reply::Text(t) => {
                    if let Some(i) = self.matching(q, &t, keys) {
                        return self.pick(q, i, How::Typed);
                    }
                    misses += 1;
                    let said = if q.irreversible {
                        self.msg(&cat::PICK_YES, &[])
                    } else {
                        let keys: Vec<String> = q
                            .choices
                            .iter()
                            .map(|c| c.key.to_ascii_lowercase().to_string())
                            .collect();
                        self.msg(&cat::PICK_ONE, &[&keys.join(" ")])
                    };
                    self.write(Stream::Err, &format!("  {said}\n"));
                }
            }
        }
        self.give_up(q)
    }

    /// The choice a typed answer names. A key press is one key; a typed line is a key or a whole
    /// label; an irreversible question takes only `yes` for yes, and its refusals by key or label.
    fn matching(&self, q: &Question, typed: &str, key_press: bool) -> Option<usize> {
        let t = typed.to_ascii_lowercase();
        if q.irreversible {
            if t == "yes" {
                return q.choices.iter().position(|c| c.affirmative);
            }
            return q
                .choices
                .iter()
                .position(|c| c.refusal && (c.label.to_ascii_lowercase() == t || is_key(c, &t)));
        }
        q.choices
            .iter()
            .position(|c| is_key(c, &t) || (!key_press && c.label.to_ascii_lowercase() == t))
    }

    /// Nothing was answered usefully: say so, change nothing, and exit 3 (SPEC 11.2). On the real
    /// terminal the process ends here, so no caller can take a default by mistake; a captured
    /// `Ui` returns the answer for the test to read.
    fn give_up(&self, q: &Question) -> Answer {
        self.chatter(&self.msg(&cat::GAVE_UP, &[]));
        if self.is_std() {
            std::process::exit(EXIT_USAGE);
        }
        self.pick(q, q.default, How::Aborted)
    }

    /// A note to stderr that `--quiet` does not hide (it explains why a question was not asked).
    fn chatter(&self, line: &str) {
        if self.mode == Mode::Json {
            return;
        }
        let lead = if self.mode == Mode::Plain {
            "note:"
        } else if self.ascii {
            "[-]"
        } else {
            "·"
        };
        self.write(
            Stream::Err,
            &format!(
                "{} {}\n",
                self.paint(Stream::Err, Role::Ash400, lead),
                self.glyphs(line)
            ),
        );
    }
}
