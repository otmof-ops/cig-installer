// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `bundle --carton <v> [--targets a,b] --out <file.tar>` (#16).

use std::path::Path;

use cigi_ui::InstallerError;

use super::Outcome;
use crate::cx::Cx;
use crate::flows::bundle::{self, BundleArgs};

pub fn run(
    cx: &mut Cx,
    carton: Option<&str>,
    targets: &[String],
    out: &Path,
) -> Result<Outcome, InstallerError> {
    bundle::run(
        cx,
        &BundleArgs {
            carton: carton.map(String::from),
            targets: targets.to_vec(),
            out: out.to_path_buf(),
        },
    )
    .map(Outcome::Report)
}
