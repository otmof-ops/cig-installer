// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! One module per command group. Each returns an `Outcome` for `main` to print.

pub mod bundle;
pub mod carton;
pub mod channel;
pub mod check;
pub mod config;
pub mod doctor;
pub mod gc;
pub mod history;
pub mod install;
pub mod list;
pub mod mirror;
pub mod pin;
pub mod repair;
pub mod resolve;
pub mod self_cmd;
pub mod shims;
pub mod status;
pub mod tabular;
pub mod uninstall;
pub mod which;

use cigi_ui::Report;

pub enum Outcome {
    Report(Report),
    /// Text printed as it is (`which --path`).
    Page(String),
}

impl From<Report> for Outcome {
    fn from(r: Report) -> Outcome {
        Outcome::Report(r)
    }
}
