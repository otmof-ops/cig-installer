// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0
//! `cargo xtask pins <manifest.toml>`: the compiled-in pins (SPEC 7.1, level `pinned`) from every
//! build of a manifest the maintainer has reviewed. The file's format lives in
//! `cigi_source::pins::render`, beside the code that reads it.

use cig_compat::Manifest;

/// The text of `crates/cigi-source/src/pins_generated.rs` for `manifest`.
pub fn pins_for(manifest: &str) -> Result<String, String> {
    let m = Manifest::from_str(manifest).map_err(|e| format!("the manifest: {e}"))?;
    let entries: Vec<(String, String)> = m
        .builds
        .iter()
        .flat_map(|((tool, v), b)| {
            b.artifacts.iter().map(move |(target, a)| {
                (
                    cigi_source::pins::key(tool, &v.to_string(), target),
                    a.sha256.clone(),
                )
            })
        })
        .collect();
    Ok(cigi_source::pins::render(&entries))
}
