// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0

//! Tables (#32): left-aligned text, right-aligned numbers, headers in `ash-400`, changed
//! cells in `ember`, wrapped to the terminal width by truncating the last column with `…`
//! unless `wide`. `--json` carries the data in the report, so `Ui::table` prints nothing there.

use crate::tokens::Role;
use crate::ui::{Mode, Stream, Ui};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
}

#[derive(Debug, Clone)]
pub struct Column {
    pub header: String,
    pub align: Align,
}

#[derive(Debug, Clone)]
pub struct Cell {
    pub text: String,
    pub changed: bool,
}

impl Cell {
    pub fn new(text: impl Into<String>) -> Cell {
        Cell {
            text: text.into(),
            changed: false,
        }
    }
    pub fn changed(text: impl Into<String>) -> Cell {
        Cell {
            text: text.into(),
            changed: true,
        }
    }
}

impl From<&str> for Cell {
    fn from(s: &str) -> Cell {
        Cell::new(s)
    }
}

impl From<String> for Cell {
    fn from(s: String) -> Cell {
        Cell::new(s)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Table {
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<Cell>>,
    /// `--wide`: never truncate.
    pub wide: bool,
}

impl Table {
    pub fn new(columns: &[(&str, Align)]) -> Table {
        Table {
            columns: columns
                .iter()
                .map(|(h, a)| Column {
                    header: (*h).to_string(),
                    align: *a,
                })
                .collect(),
            rows: Vec::new(),
            wide: false,
        }
    }
    pub fn row(mut self, cells: Vec<Cell>) -> Table {
        self.rows.push(cells);
        self
    }
    pub fn wide(mut self, wide: bool) -> Table {
        self.wide = wide;
        self
    }
}

const GAP: usize = 3;
const INDENT: usize = 2;

fn truncate(s: &str, max: usize, ellipsis: &str) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    let keep = max.saturating_sub(ellipsis.chars().count());
    s.chars().take(keep).collect::<String>() + ellipsis
}

impl Ui {
    pub fn render_table(&self, t: &Table) -> String {
        let n = t.columns.len();
        if n == 0 {
            return String::new();
        }
        let cell =
            |r: &Vec<Cell>, i: usize| r.get(i).map(|c| self.glyphs(&c.text)).unwrap_or_default();
        let mut widths: Vec<usize> = (0..n)
            .map(|i| {
                t.rows
                    .iter()
                    .map(|r| cell(r, i).chars().count())
                    .chain(std::iter::once(
                        self.glyphs(&t.columns[i].header).chars().count(),
                    ))
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        let others: usize = widths[..n - 1].iter().map(|w| w + GAP).sum();
        let room = self.width.saturating_sub(INDENT + others);
        let ellipsis = if self.ascii { "..." } else { "…" };
        if !t.wide && widths[n - 1] > room && room >= 8 {
            widths[n - 1] = room;
        }
        let s = Stream::Out;
        let put = |text: &str, i: usize, role: Option<Role>| -> String {
            let text = if i == n - 1 && !t.wide {
                truncate(text, widths[i], ellipsis)
            } else {
                text.to_string()
            };
            let len = text.chars().count();
            let pad = widths[i].saturating_sub(len);
            let painted = match role {
                Some(r) => self.paint(s, r, &text),
                None => text,
            };
            match t.columns[i].align {
                Align::Right => format!("{}{painted}", " ".repeat(pad)),
                Align::Left if i == n - 1 => painted,
                Align::Left => format!("{painted}{}", " ".repeat(pad)),
            }
        };
        let join = |parts: Vec<String>| {
            format!("{}{}\n", " ".repeat(INDENT), parts.join(&" ".repeat(GAP)))
        };
        let mut out = join(
            (0..n)
                .map(|i| put(&self.glyphs(&t.columns[i].header), i, Some(Role::Ash400)))
                .collect(),
        );
        for r in &t.rows {
            out.push_str(&join(
                (0..n)
                    .map(|i| {
                        put(
                            &cell(r, i),
                            i,
                            r.get(i).filter(|c| c.changed).map(|_| Role::Ember),
                        )
                    })
                    .collect(),
            ));
        }
        out
    }

    pub fn table(&self, t: &Table) {
        if self.mode != Mode::Json {
            self.write(Stream::Out, &self.render_table(t));
        }
    }
}
