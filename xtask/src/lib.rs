// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! Maintainer tools (`cargo xtask`): assemble the next manifest, write the compiled-in pins, write the error reference.

pub mod assemble;
pub mod carton_local;
pub mod errors_doc;
pub mod fixture_mirror;
pub mod gh;
pub mod pins;
pub mod prune;
pub mod tokens;
pub mod tuf;
