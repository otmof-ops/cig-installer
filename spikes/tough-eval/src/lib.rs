// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! Evaluation of the `tough` TUF client for decision 0004.
//!
//! `fixture` builds throwaway repositories with keys generated on the spot. `ureq_transport` shows
//! the seam: `tough` fetches through its `Transport` trait, so the installer's own HTTP client can
//! carry it and `tough`'s bundled `reqwest` stack is not needed.

pub mod fixture;
pub mod ureq_transport;
