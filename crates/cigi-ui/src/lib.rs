// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0

//! `cigi-ui`: the output grammar, palette, tables, progress, prompts, `--json` and the error
//! registry of ciginstaller (SPEC sections 2.4 and 11, issue #32). The only crate that prints.
#![forbid(unsafe_code)]

mod error;
mod grammar;
mod progress;
mod prompt;
pub mod registry;
mod table;
pub mod tokens;
mod ui;

pub use error::{
    default_exit, InstallerError, Kind, EXIT_BUG, EXIT_FAILED, EXIT_NEWER, EXIT_OK, EXIT_REFUSED,
    EXIT_SHIM, EXIT_UNREACHABLE, EXIT_USAGE,
};
pub use grammar::{finding_report, Icon, Report};
pub use progress::{render_bar, Progress, Unit};
pub use prompt::{Answer, Choice, How, Question};
pub use registry::{explain, explain_list, explain_with};
pub use table::{Align, Cell, Column, Table};
pub use tokens::{ColorMode, Role, Theme};
pub use ui::{clean, Capture, Env, GlobalFlags, Mode, ThemeChoice, Ui};
