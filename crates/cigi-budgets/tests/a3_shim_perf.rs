// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `ciginstaller/tests/a3_shim_perf.rs`, run from this package so it can be built in release mode (see this
//! package's Cargo.toml). On a release build its limits are the budgets themselves.
#![cfg(unix)]

#[path = "../../ciginstaller/tests/a3_shim_perf.rs"]
mod a3_shim_perf;
