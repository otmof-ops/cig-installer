// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! Giving a child a terminal in a test, through `script(1)`, whose arguments differ by system.

/// The arguments for `script` that run the shell line `line` on a pseudo-terminal and discard the
/// typescript. util-linux (Linux) takes `-qec <line> /dev/null`; BSD `script` (macOS) takes
/// `-q /dev/null <command> [args]`, so the line goes through `sh -c`.
pub fn script_args(line: &str) -> Vec<String> {
    if cfg!(target_os = "macos") {
        ["-q", "/dev/null", "/bin/sh", "-c", line]
            .map(String::from)
            .to_vec()
    } else {
        ["-qec", line, "/dev/null"].map(String::from).to_vec()
    }
}
