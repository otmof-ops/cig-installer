// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! `cigi-ui`: the output grammar, palette, tables, progress, prompts, `--json` and the error
//! registry of ciginstaller (SPEC sections 2.4 and 11, issues #31 and #32). The only crate
//! that prints.
//!
//! The palette is generated from `design/tokens.json` (`tokens.rs`, never edited by hand);
//! `paint.rs` and `tokens.rs` together are the painter meant to move into the family's shared
//! `cig-paint` crate.
#![forbid(unsafe_code)]

pub mod catalogue;
pub mod detect;
mod error;
mod grammar;
pub mod paint;
mod progress;
mod prompt;
pub mod registry;
mod table;
pub mod tokens;
mod ui;

pub use catalogue::Msg;
pub use error::{
    default_exit, InstallerError, Kind, EXIT_BUG, EXIT_FAILED, EXIT_NEWER, EXIT_OK, EXIT_REFUSED,
    EXIT_SHIM, EXIT_UNREACHABLE, EXIT_USAGE,
};
pub use grammar::{finding_report, Icon, Report};
pub use paint::{paint, sgr, ColorMode};
pub use progress::{render_bar, Progress, Unit};
pub use prompt::{Answer, Choice, How, Question};
pub use registry::{explain, explain_list, explain_with};
pub use table::{Align, Cell, Column, Table};
pub use tokens::{Role, Theme};
pub use ui::{clean, product_names, Capture, Env, GlobalFlags, Mode, ThemeChoice, Ui, VoiceChoice};
