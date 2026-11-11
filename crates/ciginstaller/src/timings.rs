// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! The installer's phases on `CIG_TIMINGS_FD` (#35, otmof-ops/cig-script#179): when `cig bench
//! --timings` names a descriptor, each command writes where its time went as timings lines
//! (format 1) and only then. With the variable unset `begin` reads one environment variable and
//! every `mark` is a lock and a test.
//!
//! Phases, flat and in the order they ended: `start` (the process's first line to the parsed
//! command line), `home` (reading the installer's state), `command` (the command itself),
//! `output` (printing the answer). What is left of the wall time (exec, exit) is `other` in the
//! harness. A line is one `write` of far less than `PIPE_BUF`, so products sharing a
//! descriptor never mix lines.
//!
//! The descriptor is reached through `/dev/fd/N` so this crate's `forbid(unsafe_code)` holds;
//! that path exists on Linux and macOS, and a descriptor that cannot be opened is ignored,
//! never an error: a stale variable must not break a command.

use std::sync::Mutex;
use std::time::Instant;

struct State {
    fd: u32,
    last: Instant,
    lines: Vec<String>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// The descriptor a value of `CIG_TIMINGS_FD` names: digits only, 3 or more (0 to 2 are the
/// standard streams).
pub fn parse_fd(v: &str) -> Option<u32> {
    if v.is_empty() || v.len() > 4 || !v.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    v.parse().ok().filter(|n| *n >= 3)
}

/// One timings line for a phase.
pub fn line(phase: &str, ms: f64) -> String {
    format!("{{\"v\":1,\"product\":\"ciginstaller\",\"phase\":\"{phase}\",\"ms\":{ms:.3}}}")
}

/// Start the clock when `CIG_TIMINGS_FD` names a descriptor. Call it first in `main`.
pub fn begin() {
    let Some(fd) = std::env::var("CIG_TIMINGS_FD")
        .ok()
        .and_then(|v| parse_fd(&v))
    else {
        return;
    };
    // our own children (a tool the installer runs) must not write the same phases again
    std::env::remove_var("CIG_TIMINGS_FD");
    if let Ok(mut s) = STATE.lock() {
        *s = Some(State {
            fd,
            last: Instant::now(),
            lines: Vec::new(),
        });
    }
}

/// Close the phase that has just ended, named `phase`.
pub fn mark(phase: &str) {
    if let Ok(mut s) = STATE.lock() {
        if let Some(st) = s.as_mut() {
            let now = Instant::now();
            let ms = now.duration_since(st.last).as_secs_f64() * 1000.0;
            st.last = now;
            st.lines.push(line(phase, ms));
        }
    }
}

/// Write the lines, once, and stop.
pub fn finish() {
    let Some(st) = STATE.lock().ok().and_then(|mut s| s.take()) else {
        return;
    };
    if st.lines.is_empty() {
        return;
    }
    #[cfg(unix)]
    {
        use std::io::Write;
        let mut body = st.lines.join("\n");
        body.push('\n');
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .append(true)
            .open(format!("/dev/fd/{}", st.fd))
        {
            let _ = f.write_all(body.as_bytes());
        }
    }
    #[cfg(not(unix))]
    let _ = st.fd;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_real_descriptor_is_taken() {
        assert_eq!(parse_fd("9"), Some(9));
        assert_eq!(parse_fd("12"), Some(12));
        for bad in ["", "0", "1", "2", "-1", "9 ", "x", "99999", "9;ls"] {
            assert_eq!(parse_fd(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_line_is_the_v1_format() {
        let v: serde_json::Value = serde_json::from_str(&line("home", 1.5)).unwrap();
        assert_eq!(v["v"], 1);
        assert_eq!(v["product"], "ciginstaller");
        assert_eq!(v["phase"], "home");
        assert_eq!(v["ms"], 1.5);
    }
}
