// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `history`: what this machine has been through, newest first. Read-only: `state.json` keeps
//! the last 50 operations and this only shows them.

use cigi_ui::{Align, Cell, InstallerError, Report, Table};
use serde_json::json;

use super::tabular::details;
use super::Outcome;
use crate::cx::Cx;

pub fn run(cx: &mut Cx) -> Result<Outcome, InstallerError> {
    let h = &cx.state.history;
    if h.is_empty() {
        return Ok(Report::done("nothing has been installed or changed yet")
            .next("ciginstaller install carton")
            .data(json!({ "entries": [] }))
            .into());
    }
    let mut table = Table::new(&[
        ("id", Align::Left),
        ("when", Align::Left),
        ("what", Align::Left),
        ("from", Align::Left),
        ("to", Align::Left),
        ("ok", Align::Left),
    ]);
    let mut entries = Vec::new();
    for e in h.iter().rev() {
        let dash = |v: &Option<String>| v.clone().unwrap_or_else(|| "-".to_string());
        table = table.row(vec![
            Cell::new(e.id.clone()),
            Cell::new(e.at.clone()),
            Cell::new(e.what.clone()),
            Cell::new(dash(&e.from)),
            Cell::new(dash(&e.to)),
            if e.ok {
                Cell::new("yes")
            } else {
                Cell::changed("no")
            },
        ]);
        entries.push(json!({
            "id": e.id, "at": e.at, "what": e.what, "from": e.from, "to": e.to,
            "ok": e.ok, "txn": e.txn,
        }));
    }
    let mut r = Report::done(format!("{} operation(s), newest first", h.len()));
    for l in details(&cx.ui, &table) {
        r = r.detail(l);
    }
    if cx.state.previous.machine_carton.is_some() {
        r = r.next("ciginstaller rollback");
    }
    Ok(r.data(json!({ "entries": entries })).into())
}
