// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! The lowest `snapshot` and `targets` versions this build will ever accept (decision 0004,
//! section 5). A machine with no history accepts any unexpired, correctly signed metadata, so a
//! build carries its own floor: written after each signing sitting from the metadata just signed
//! (`cargo xtask tuf floor <metadata-dir>` prints these two lines).

pub const MIN_SNAPSHOT_VERSION: u64 = 1;
pub const MIN_TARGETS_VERSION: u64 = 1;
