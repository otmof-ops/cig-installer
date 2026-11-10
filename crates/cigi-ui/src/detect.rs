// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! Reading the terminal (#32): which background it has (`COLORFGBG`, then the OSC 11 query
//! with a hard timeout), and the one-keypress read. No `unsafe` and no terminal crate:
//! terminal modes are changed by `stty`, the same tool a shell script would use, and every
//! child process runs under a deadline, so nothing here can hang a command, a test or a pipe.
//!
//! Both are used only on a real terminal and only from `Ui::from_env_and_flags` and
//! `Ui::ask`; the pure `Ui::from_env` and everything a test builds never touch the terminal.

use crate::tokens::Theme;
use std::time::Duration;

pub type Rgb = (u8, u8, u8);

/// `COLORFGBG` is `fg;bg` (sometimes `fg;default;bg`); the last field is the background's
/// ANSI index. 7 and 9-15 are light backgrounds, 0-6 and 8 dark.
pub fn parse_colorfgbg(v: &str) -> Option<Theme> {
    let bg: u8 = v.rsplit(';').next()?.trim().parse().ok()?;
    Some(if bg == 7 || (9..=15).contains(&bg) {
        Theme::Light
    } else {
        Theme::Dark
    })
}

/// WCAG relative luminance of an sRGB colour.
pub fn luminance(c: Rgb) -> f64 {
    let lin = |v: u8| {
        let s = f64::from(v) / 255.0;
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c.0) + 0.7152 * lin(c.1) + 0.0722 * lin(c.2)
}

/// WCAG contrast ratio, 1 to 21.
pub fn contrast(a: Rgb, b: Rgb) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// A background is light when black text on it beats white text on it.
pub fn theme_of_background(c: Rgb) -> Theme {
    if luminance(c) > 0.179 {
        Theme::Light
    } else {
        Theme::Dark
    }
}

/// The colour in an OSC 11 reply: `ESC ] 11 ; rgb:RRRR/GGGG/BBBB` ended by BEL or `ESC \`,
/// each channel one to four hex digits.
pub fn parse_osc11(reply: &[u8]) -> Option<Rgb> {
    let text = String::from_utf8_lossy(reply);
    let at = text.find("rgb:")? + 4;
    let body: String = text[at..]
        .chars()
        .take_while(|c| c.is_ascii_hexdigit() || *c == '/')
        .collect();
    let mut channels = body.split('/');
    let mut next = || {
        let h = channels.next()?;
        if h.is_empty() || h.len() > 4 {
            return None;
        }
        let v = u32::from_str_radix(h, 16).ok()?;
        let max = 16u32.pow(h.len() as u32) - 1;
        u8::try_from((v * 255 + max / 2) / max).ok()
    };
    Some((next()?, next()?, next()?))
}

/// The device-attributes reply (`ESC [ ? ... c`) that ends the query: terminals answer it
/// even when they ignore OSC 11, so an unsupportive terminal costs a few milliseconds, not
/// the whole timeout.
fn answered_da1(buf: &[u8]) -> bool {
    buf.windows(3)
        .position(|w| w == b"\x1b[?")
        .is_some_and(|i| buf[i..].contains(&b'c'))
}

/// Ask the terminal for its background colour: `ESC ] 11 ; ? BEL` followed by a
/// device-attributes request, with a hard `timeout` (50 ms by default in the installer).
/// `None` when there is no terminal, it does not answer in time, or anything goes wrong.
#[cfg(unix)]
pub fn query_background(timeout: Duration) -> Option<Rgb> {
    use std::fs::{File, OpenOptions};
    use std::io::{Read, Write};
    use std::process::{Child, Command, Stdio};
    use std::time::Instant;

    let start = Instant::now();
    let hard = start + timeout;
    let mut tty = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .ok()?;

    fn wait(mut child: Child, until: Instant) -> Option<(bool, Vec<u8>)> {
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let mut out = Vec::new();
                    if let Some(mut o) = child.stdout.take() {
                        let _ = o.read_to_end(&mut out);
                    }
                    return Some((status.success(), out));
                }
                Ok(None) if Instant::now() < until => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
            }
        }
    }
    let stty = |tty: &File, args: &[&str], until: Instant| -> Option<String> {
        let child = Command::new("stty")
            .args(args)
            .stdin(Stdio::from(tty.try_clone().ok()?))
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let (ok, out) = wait(child, until)?;
        ok.then(|| String::from_utf8_lossy(&out).trim().to_string())
    };

    let saved = stty(&tty, &["-g"], hard)?;
    // Restoring may run a little past the query's deadline; it is bounded all the same.
    let restore_by = hard + Duration::from_millis(50);
    if stty(&tty, &["-icanon", "-echo", "min", "0", "time", "1"], hard).is_none() {
        let _ = stty(&tty, &[&saved], restore_by);
        return None;
    }
    let mut reply = Vec::new();
    let spawned = Command::new("dd")
        .args(["bs=1", "count=96"])
        .stdin(Stdio::from(tty.try_clone().ok()?))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    if let Ok(mut dd) = spawned {
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        if let Some(mut out) = dd.stdout.take() {
            std::thread::spawn(move || {
                let mut b = [0u8; 64];
                while let Ok(n) = out.read(&mut b) {
                    if n == 0 || tx.send(b[..n].to_vec()).is_err() {
                        break;
                    }
                }
            });
        }
        if tty
            .write_all(b"\x1b]11;?\x07\x1b[c")
            .and_then(|()| tty.flush())
            .is_ok()
        {
            while let Some(left) = hard.checked_duration_since(Instant::now()) {
                match rx.recv_timeout(left) {
                    Ok(chunk) => {
                        reply.extend_from_slice(&chunk);
                        if answered_da1(&reply) {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        }
        let _ = dd.kill();
        let _ = dd.wait();
    }
    let _ = stty(&tty, &[&saved], restore_by);
    parse_osc11(&reply)
}

#[cfg(not(unix))]
pub fn query_background(_timeout: Duration) -> Option<Rgb> {
    None
}

/// One keypress from the terminal, without enter. The terminal mode is changed and restored
/// by a small `sh` script that traps INT/TERM/HUP, so a Ctrl-C at the prompt cannot leave the
/// terminal without echo. `None` when it cannot be done (no `sh`, `stty` or `dd`); the caller
/// then reads a line instead. The key comes back as its first byte, so only ASCII keys count.
#[cfg(unix)]
pub fn read_key() -> Option<Option<char>> {
    use std::process::{Command, Stdio};
    let script = "saved=$(stty -g) || exit 1\n\
        trap 'stty \"$saved\"' EXIT\n\
        trap 'exit 130' INT TERM HUP\n\
        stty -icanon -echo min 1 time 0 || exit 1\n\
        dd bs=1 count=1 2>/dev/null | od -An -tx1\n";
    let out = Command::new("sh")
        .arg("-c")
        .arg(script)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let hex = String::from_utf8_lossy(&out.stdout);
    let byte = u8::from_str_radix(hex.split_whitespace().next()?, 16).ok()?;
    Some(match byte {
        b'\n' | b'\r' => None,
        b if b.is_ascii_graphic() => Some(char::from(b)),
        _ => Some('\u{0}'),
    })
}

#[cfg(not(unix))]
pub fn read_key() -> Option<Option<char>> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colorfgbg_last_field_is_the_background() {
        assert_eq!(parse_colorfgbg("0;15"), Some(Theme::Light));
        assert_eq!(parse_colorfgbg("15;0"), Some(Theme::Dark));
        assert_eq!(parse_colorfgbg("12;default;7"), Some(Theme::Light));
        assert_eq!(parse_colorfgbg("7;8"), Some(Theme::Dark));
        assert_eq!(parse_colorfgbg("nonsense"), None);
        assert_eq!(parse_colorfgbg(""), None);
    }

    #[test]
    fn osc11_replies_parse_in_every_shape() {
        let bel = b"\x1b]11;rgb:1e1e/2020/2424\x07\x1b[?62;c";
        assert_eq!(parse_osc11(bel), Some((0x1e, 0x20, 0x24)));
        let st = b"\x1b]11;rgb:ffff/ffff/ffff\x1b\\";
        assert_eq!(parse_osc11(st), Some((255, 255, 255)));
        assert_eq!(parse_osc11(b"\x1b]11;rgb:0/8/f\x07"), Some((0, 136, 255)));
        assert_eq!(parse_osc11(b"\x1b[?62;c"), None);
        assert_eq!(parse_osc11(b"\x1b]11;rgb:zz/00/00\x07"), None);
        assert_eq!(parse_osc11(b"\x1b]11;rgb:fffff/00/00\x07"), None);
        assert_eq!(parse_osc11(b""), None);
    }

    #[test]
    fn backgrounds_are_told_apart() {
        assert_eq!(theme_of_background((255, 255, 255)), Theme::Light);
        assert_eq!(theme_of_background((0x0d, 0x11, 0x17)), Theme::Dark);
        assert_eq!(theme_of_background((0xfd, 0xf6, 0xe3)), Theme::Light);
        assert_eq!(theme_of_background((0x1e, 0x1e, 0x2e)), Theme::Dark);
    }

    #[test]
    fn contrast_matches_wcag() {
        assert!((contrast((0, 0, 0), (255, 255, 255)) - 21.0).abs() < 1e-9);
        assert!((contrast((255, 255, 255), (255, 255, 255)) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn the_device_attributes_reply_ends_the_wait() {
        assert!(answered_da1(b"\x1b]11;rgb:0/0/0\x07\x1b[?62;4c"));
        assert!(!answered_da1(b"\x1b]11;rgb:0/0/0\x07"));
        assert!(!answered_da1(b"\x1b[?62;4"));
    }
}
