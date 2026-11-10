// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! Progress (#32): one bar at a time, in `ember`; a line per 25% when output is not a
//! terminal; a line at the start and the end only when `--plain` is on at a terminal (a
//! screen reader); nothing in Json mode or under `--quiet`.
//!
//! `Progress` is defined here because `cigi-ui` returns it; the SPEC's graph has `cigi-source`
//! taking `&mut dyn Progress` without depending on this crate, so integration must move the
//! trait to a crate both see (the report proposes `cig-compat`).

use crate::tokens::Role;
use crate::ui::{Mode, Stream, Ui};
use std::time::{Duration, Instant};

pub trait Progress {
    fn set_total(&mut self, total: u64);
    fn advance(&mut self, by: u64);
    fn set_position(&mut self, to: u64);
    /// Done: the bar collapses to a `✓ summary` line (the label when `None`).
    fn finish(&mut self, summary: Option<&str>);
    /// Gave up: the bar is cleared, nothing is claimed.
    fn abort(&mut self);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Bytes,
    Count(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Style {
    Silent,
    Live,
    StartEnd,
    Quartiles,
}

fn bytes(done: u64, total: Option<u64>) -> String {
    let basis = total.unwrap_or(done);
    let (div, unit) = match basis {
        b if b >= 1_000_000_000 => (1e9, "GB"),
        b if b >= 1_000_000 => (1e6, "MB"),
        b if b >= 1_000 => (1e3, "kB"),
        _ => (1.0, "B"),
    };
    match total {
        Some(t) => format!("{:.1}/{:.1} {unit}", done as f64 / div, t as f64 / div),
        None => format!("{:.1} {unit}", done as f64 / div),
    }
}

fn clock(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 60 {
        format!("{} m {:02} s", s / 60, s % 60)
    } else {
        format!("{s} s")
    }
}

/// The live bar line (no escape codes unless colour is on), as drawn at a terminal.
pub fn render_bar(
    ui: &Ui,
    label: &str,
    unit: Unit,
    done: u64,
    total: Option<u64>,
    elapsed: Duration,
) -> String {
    let s = Stream::Err;
    let amount = match unit {
        Unit::Bytes => bytes(done, total),
        Unit::Count(noun) => match total {
            Some(t) => format!("{done}/{t} {noun}"),
            None => format!("{done} {noun}"),
        },
    };
    let rate = match unit {
        Unit::Bytes if elapsed.as_millis() >= 500 && done > 0 => {
            let per = done as f64 / elapsed.as_secs_f64();
            let (d, u) = if per >= 1e6 {
                (1e6, "MB/s")
            } else {
                (1e3, "kB/s")
            };
            format!("  {:.1} {u}", per / d)
        }
        _ => String::new(),
    };
    let tail = format!("  {amount}{rate}  {}", clock(elapsed));
    let icon = if ui.ascii { "[..]" } else { "●" };
    let used = icon.chars().count() + 1 + label.chars().count() + 2 + tail.chars().count() + 2;
    let mut line = format!("{} {label}", ui.paint(s, Role::EmberBright, icon));
    if let Some(t) = total.filter(|t| *t > 0) {
        let n = ui.width.saturating_sub(used).clamp(10, 26);
        let frac = (done.min(t) as f64) / (t as f64);
        let full = ((frac * n as f64) as usize).min(n);
        let bar = if ui.ascii {
            format!("[{}{}]", "#".repeat(full), "-".repeat(n - full))
        } else if full >= n {
            ui.paint(s, Role::Ember, &"━".repeat(n))
        } else {
            format!(
                "{}{}",
                ui.paint(s, Role::Ember, &format!("{}╸", "━".repeat(full))),
                ui.paint(s, Role::Ash400, &"━".repeat(n - full - 1))
            )
        };
        line.push_str(&format!("  {bar}"));
    }
    line.push_str(&tail);
    line
}

/// The text without colour escapes (for measuring).
fn strip_sgr(s: &str) -> String {
    let mut out = String::new();
    let mut skipping = false;
    for c in s.chars() {
        match (skipping, c) {
            (_, '\x1b') => skipping = true,
            (true, 'm') => skipping = false,
            (false, c) => out.push(c),
            _ => {}
        }
    }
    out
}

struct Bar {
    ui: Ui,
    label: String,
    unit: Unit,
    style: Style,
    total: Option<u64>,
    done: u64,
    start: Instant,
    last_draw: Instant,
    bucket: Option<u8>,
    drawn: bool,
    /// Visible width of the last live line, so a shorter one can blank the rest (no escape codes).
    last_len: usize,
    over: bool,
}

impl Bar {
    fn prefix(&self) -> String {
        if self.ui.mode == Mode::Plain {
            self.ui.paint(Stream::Err, Role::EmberBright, "...")
        } else {
            self.ui.paint(
                Stream::Err,
                Role::EmberBright,
                if self.ui.ascii { "[..]" } else { "●" },
            )
        }
    }

    fn percent_line(&mut self, pct: u8) {
        let amount = match (self.unit, self.total) {
            (Unit::Bytes, t) => bytes(self.done, t),
            (Unit::Count(n), Some(t)) => format!("{}/{t} {n}", self.done),
            (Unit::Count(n), None) => format!("{} {n}", self.done),
        };
        self.ui.write(
            Stream::Err,
            &format!("{} {}  {pct}%  ({amount})\n", self.prefix(), self.label),
        );
        self.bucket = Some(pct);
    }

    fn update(&mut self) {
        match self.style {
            Style::Silent | Style::StartEnd => {}
            Style::Live => {
                if self.last_draw.elapsed() >= Duration::from_millis(100) {
                    self.draw();
                }
            }
            Style::Quartiles => {
                let Some(t) = self.total.filter(|t| *t > 0) else {
                    return;
                };
                let pct = ((self.done.min(t) * 100) / t) as u8;
                let bucket = pct / 25 * 25;
                if bucket < 100 && self.bucket.is_none_or(|b| bucket > b) {
                    self.percent_line(bucket);
                }
            }
        }
    }

    fn draw(&mut self) {
        let line = render_bar(
            &self.ui,
            &self.label,
            self.unit,
            self.done,
            self.total,
            self.start.elapsed(),
        );
        let visible = strip_sgr(&line).chars().count();
        let blank = " ".repeat(self.last_len.saturating_sub(visible));
        self.ui.write(Stream::Err, &format!("\r{line}{blank}"));
        self.last_len = visible;
        self.last_draw = Instant::now();
        self.drawn = true;
    }

    fn clear(&mut self) {
        if self.drawn {
            self.ui
                .write(Stream::Err, &format!("\r{}\r", " ".repeat(self.last_len)));
            self.drawn = false;
            self.last_len = 0;
        }
    }
}

impl Progress for Bar {
    fn set_total(&mut self, total: u64) {
        self.total = Some(total);
        self.update();
    }

    fn advance(&mut self, by: u64) {
        self.done = self.done.saturating_add(by);
        self.update();
    }

    fn set_position(&mut self, to: u64) {
        self.done = to;
        self.update();
    }

    fn finish(&mut self, summary: Option<&str>) {
        if self.over {
            return;
        }
        self.over = true;
        let said = summary.map_or_else(|| self.label.clone(), str::to_string);
        let done_line = |ui: &Ui, said: &str| {
            let (g, text) = if ui.mode == Mode::Plain {
                ("ok:", ui.glyphs(said))
            } else {
                (if ui.ascii { "[ok]" } else { "✓" }, ui.glyphs(said))
            };
            format!("{} {text}\n", ui.paint(Stream::Err, Role::Ok, g))
        };
        match self.style {
            Style::Silent => {}
            Style::Live => {
                self.clear();
                self.ui.write(Stream::Err, &done_line(&self.ui, &said));
            }
            Style::StartEnd => self.ui.write(Stream::Err, &done_line(&self.ui, &said)),
            Style::Quartiles => {
                if let Some(t) = self.total.filter(|t| *t > 0) {
                    self.done = t;
                    if self.bucket != Some(100) {
                        self.percent_line(100);
                    }
                }
                if summary.is_some() {
                    self.ui.write(Stream::Err, &done_line(&self.ui, &said));
                }
            }
        }
    }

    fn abort(&mut self) {
        self.over = true;
        if self.style == Style::Live {
            self.clear();
        }
    }
}

impl Drop for Bar {
    fn drop(&mut self) {
        if !self.over && self.style == Style::Live {
            self.clear();
        }
    }
}

impl Ui {
    /// A byte-counting bar. `total` is the size when it is known.
    pub fn progress(&self, label: &str, total: Option<u64>) -> Box<dyn Progress> {
        self.bar(label, total, Unit::Bytes)
    }

    /// A bar that counts things (`9/14 scripts`).
    pub fn progress_counted(
        &self,
        label: &str,
        total: Option<u64>,
        noun: &'static str,
    ) -> Box<dyn Progress> {
        self.bar(label, total, Unit::Count(noun))
    }

    fn bar(&self, label: &str, total: Option<u64>, unit: Unit) -> Box<dyn Progress> {
        let style = if self.mode == Mode::Json || self.quiet {
            Style::Silent
        } else if self.tty_err && !self.term_dumb {
            if self.mode == Mode::Plain {
                Style::StartEnd
            } else {
                Style::Live
            }
        } else {
            Style::Quartiles
        };
        let now = Instant::now();
        let mut bar = Bar {
            ui: self.duplicate(),
            label: self.glyphs(label),
            unit,
            style,
            total,
            done: 0,
            start: now,
            last_draw: now,
            bucket: None,
            drawn: false,
            last_len: 0,
            over: false,
        };
        match style {
            Style::Live => bar.draw(),
            Style::StartEnd => bar
                .ui
                .write(Stream::Err, &format!("{} {}\n", bar.prefix(), bar.label)),
            Style::Quartiles => match total.filter(|t| *t > 0) {
                Some(_) => bar.percent_line(0),
                None => bar
                    .ui
                    .write(Stream::Err, &format!("{} {}\n", bar.prefix(), bar.label)),
            },
            Style::Silent => {}
        }
        Box::new(bar)
    }
}
