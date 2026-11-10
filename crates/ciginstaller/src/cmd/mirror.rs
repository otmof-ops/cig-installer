// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `mirror <dir> [--carton <v>] [--channel <c>] [--targets a,b]` (#16).

use std::path::Path;

use cigi_ui::InstallerError;

use super::install::no_such_channel;
use super::Outcome;
use crate::cx::Cx;
use crate::flows::mirror::{self, MirrorArgs};

pub fn run(
    cx: &mut Cx,
    dir: &Path,
    carton: Option<&str>,
    channel: Option<&str>,
    targets: &[String],
) -> Result<Outcome, InstallerError> {
    if let Some(ch) = channel {
        cx.cfg
            .set_for_run("channel", ch)
            .map_err(|_| no_such_channel(ch))?;
    }
    mirror::run(
        cx,
        &MirrorArgs {
            dir: dir.to_path_buf(),
            carton: carton.map(String::from),
            targets: targets.to_vec(),
        },
    )
    .map(Outcome::Report)
}
