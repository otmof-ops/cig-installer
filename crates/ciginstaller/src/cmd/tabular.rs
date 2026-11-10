// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! A table as the detail lines of a report, so the headline comes first and the grammar's
//! indent holds. Under `--json` the report's data carries the rows and the lines are left out.

use cigi_ui::{Mode, Table, Ui};

pub fn details(ui: &Ui, t: &Table) -> Vec<String> {
    if ui.mode == Mode::Json {
        return Vec::new();
    }
    ui.render_table(t)
        .lines()
        .map(|l| l.strip_prefix("  ").unwrap_or(l).to_string())
        .collect()
}
