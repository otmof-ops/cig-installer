// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! Reading the terminal (#32): which background it has (`COLORFGBG`, then the OSC 11 query
//! with a hard timeout), the one-keypress read, and draining typed-ahead input. No `unsafe`
//! and no terminal crate: terminal modes are changed by `stty`, the same tool a shell script
//! would use. The helpers (`sh`, `stty`, `dd`, `od`, `wc`, `kill`) are run by absolute path
//! (`/bin`, then `/usr/bin`) with a cleared environment that keeps only `TERM`, so a hostile
//! `PATH` cannot substitute them; each runs one `/bin/sh` script whose trap restores the
//! terminal on INT, TERM and HUP, and a drop guard restores it if this process panics; every
//! child runs under a deadline, so nothing here can hang a command, a test or a pipe.
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

/// What a keypress was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// A printable ASCII key.
    Char(char),
    /// Enter.
    Enter,
    /// Ctrl-D: no more input.
    Eof,
    /// Not an answer: an escape sequence (an arrow key, a late terminal reply) or another control byte.
    Ignored,
}

/// The meaning of the first byte of a keypress. A byte that starts an ESC sequence is never an
/// answer, and neither is any other control byte.
pub fn key_of(byte: u8) -> Key {
    match byte {
        b'\n' | b'\r' => Key::Enter,
        0x04 => Key::Eof,
        b if b.is_ascii_graphic() => Key::Char(char::from(b)),
        _ => Key::Ignored,
    }
}

/// The device-attributes reply (`ESC [ ? ... c`) that ends the query: terminals answer it
/// even when they ignore OSC 11, so an unsupportive terminal costs a few milliseconds, not
/// the whole timeout.
#[cfg_attr(not(unix), allow(dead_code))] // only the Unix terminal query reads it
fn answered_da1(buf: &[u8]) -> bool {
    buf.windows(3)
        .position(|w| w == b"\x1b[?")
        .is_some_and(|i| buf[i..].contains(&b'c'))
}

/// Is this process in the foreground process group of its terminal? Reads `tpgid` and `pgrp`
/// from `/proc/self/stat` (Linux). Where there is no `/proc`, the answer is yes.
pub fn in_foreground() -> bool {
    // only Linux has `/proc/self/stat`; elsewhere there is nothing to read, so the answer is yes
    if !cfg!(target_os = "linux") {
        return true;
    }
    match std::fs::read_to_string("/proc/self/stat") {
        Ok(stat) => foreground_in_stat(&stat).unwrap_or(false),
        Err(_) => !std::path::Path::new("/proc/self").exists(),
    }
}

/// `tpgid == pgrp` in the text of `/proc/<pid>/stat`.
pub fn foreground_in_stat(stat: &str) -> Option<bool> {
    // pid (comm) state ppid pgrp session tty_nr tpgid ...: the command may hold spaces and parens
    let rest = &stat[stat.rfind(')')? + 1..];
    let f: Vec<&str> = rest.split_whitespace().collect();
    let pgrp: i64 = f.get(2)?.parse().ok()?;
    let tpgid: i64 = f.get(5)?.parse().ok()?;
    Some(tpgid > 0 && tpgid == pgrp)
}

/// A value from `stty -g` is safe to hand back to `stty` as a single word: not empty, no
/// leading `-` (it would be read as an option), and only the characters `stty -g` prints.
pub fn safe_stty_state(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || ":=,._-".contains(c))
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::fs::{File, OpenOptions};
    use std::io::Read;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::time::Instant;

    /// A helper program by absolute path: `/bin`, then `/usr/bin`. Never searched on `PATH`.
    pub fn tool(name: &str) -> Option<PathBuf> {
        ["/bin", "/usr/bin"]
            .iter()
            .map(|d| Path::new(d).join(name))
            .find(|p| p.is_file())
    }

    /// `/bin/sh -c script sh args...` with an empty environment but `TERM`.
    fn sh(script: &str, args: &[&Path]) -> Option<Command> {
        let mut c = Command::new(tool("sh")?);
        c.env_clear();
        if let Some(t) = std::env::var_os("TERM") {
            c.env("TERM", t);
        }
        c.arg("-c").arg(script).arg("sh");
        for a in args {
            c.arg(a);
        }
        Some(c)
    }

    fn wait_until(child: &mut Child, until: Instant) -> Option<(bool, Vec<u8>)> {
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

    fn stty(stty: &Path, tty: &File, args: &[&str], until: Instant) -> Option<String> {
        let mut child = Command::new(stty)
            .env_clear()
            .args(args)
            .stdin(Stdio::from(tty.try_clone().ok()?))
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let (ok, out) = wait_until(&mut child, until)?;
        ok.then(|| String::from_utf8_lossy(&out).trim().to_string())
    }

    /// Puts the terminal back as it was, when dropped: normally, or while a panic unwinds.
    struct Restore<'a> {
        stty: &'a Path,
        tty: &'a File,
        saved: String,
    }

    impl Drop for Restore<'_> {
        fn drop(&mut self) {
            if safe_stty_state(&self.saved) {
                let until = Instant::now() + Duration::from_millis(100);
                let _ = stty(self.stty, self.tty, &[self.saved.as_str()], until);
            }
        }
    }

    const PROBE: &str = "STTY=$1; DD=$2; saved=$3\n\
        case \"$saved\" in ''|-*) exit 1;; esac\n\
        trap '$STTY \"$saved\" </dev/tty' EXIT\n\
        trap 'exit 130' INT TERM HUP\n\
        $STTY -icanon -echo min 0 time 0 </dev/tty || exit 1\n\
        $DD bs=4096 count=1 </dev/tty >/dev/null 2>&1\n\
        $STTY min 0 time 1 </dev/tty || exit 1\n\
        $DD bs=1 count=96 </dev/tty 2>/dev/null &\n\
        dpid=$!\n\
        echo \"pid=$dpid\"\n\
        printf '\\033]11;?\\007\\033[c' >/dev/tty\n\
        wait $dpid\n\
        $STTY min 0 time 0 </dev/tty\n\
        $DD bs=4096 count=1 </dev/tty >/dev/null 2>&1\n";

    /// Ask the terminal for its background colour: `ESC ] 11 ; ? BEL` followed by a
    /// device-attributes request, with a hard `timeout`. `None` when there is no terminal, it
    /// does not answer in time, or anything goes wrong. Pending input is drained before the
    /// terminal is restored, so a late reply is not typed into the next prompt.
    pub fn query_background(timeout: Duration) -> Option<Rgb> {
        let start = Instant::now();
        let hard = start + timeout;
        let (stty_path, dd, kill) = (tool("stty")?, tool("dd")?, tool("kill"));
        let tty = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .ok()?;
        let saved = stty(&stty_path, &tty, &["-g"], hard)?;
        if !safe_stty_state(&saved) {
            return None;
        }
        let _guard = Restore {
            stty: &stty_path,
            tty: &tty,
            saved: saved.clone(),
        };
        let mut child = sh(PROBE, &[&stty_path, &dd, Path::new(&saved)])?
            .stdin(Stdio::from(tty.try_clone().ok()?))
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        if let Some(mut out) = child.stdout.take() {
            std::thread::spawn(move || {
                let mut b = [0u8; 64];
                while let Ok(n) = out.read(&mut b) {
                    if n == 0 || tx.send(b[..n].to_vec()).is_err() {
                        break;
                    }
                }
            });
        }
        let mut got = Vec::new();
        while let Some(left) = hard.checked_duration_since(Instant::now()) {
            match rx.recv_timeout(left) {
                Ok(chunk) => {
                    got.extend_from_slice(&chunk);
                    if answered_da1(&got) {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        // stop the reader by pid (SIGTERM): the script's trap then restores the terminal
        let pid = String::from_utf8_lossy(&got)
            .lines()
            .find_map(|l| l.strip_prefix("pid=").map(str::to_string))
            .filter(|p| p.chars().all(|c| c.is_ascii_digit()));
        let stop = |sig: &str| {
            if let (Some(k), Some(p)) = (&kill, &pid) {
                let _ = Command::new(k)
                    .env_clear()
                    .args([sig, p.as_str()])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
        };
        stop("-TERM");
        let end = Instant::now() + Duration::from_millis(150);
        while child.try_wait().ok().flatten().is_none() {
            if Instant::now() >= end {
                stop("-KILL");
                let _ = child.kill();
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        let _ = child.wait();
        // the guard restores once more as this returns
        let reply = &got[got.iter().position(|b| *b == b'\n').map_or(0, |i| i + 1)..];
        parse_osc11(reply)
    }

    const DRAIN: &str = "STTY=$1; DD=$2\n\
        saved=$($STTY -g) || exit 1\n\
        case \"$saved\" in ''|-*) exit 1;; esac\n\
        trap '$STTY \"$saved\"' EXIT\n\
        trap 'exit 130' INT TERM HUP\n\
        $STTY -icanon min 0 time 0 || exit 1\n\
        $DD bs=4096 count=1 >/dev/null 2>&1\n";

    /// Throw away whatever was typed ahead of a question. `false` when it could not be done.
    pub fn drain_input() -> bool {
        let (Some(s), Some(d)) = (tool("stty"), tool("dd")) else {
            return false;
        };
        let Some(mut cmd) = sh(DRAIN, &[&s, &d]) else {
            return false;
        };
        let Ok(mut child) = cmd
            .stdin(Stdio::inherit())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        else {
            return false;
        };
        wait_until(&mut child, Instant::now() + Duration::from_millis(500))
            .is_some_and(|(ok, _)| ok)
    }

    const KEY: &str = "STTY=$1; DD=$2; OD=$3; WC=$4\n\
        saved=$($STTY -g) || exit 1\n\
        case \"$saved\" in ''|-*) exit 1;; esac\n\
        trap '$STTY \"$saved\"' EXIT\n\
        trap 'exit 130' INT TERM HUP\n\
        $STTY -icanon -echo min 0 time 0 || exit 1\n\
        $DD bs=4096 count=1 >/dev/null 2>&1\n\
        $STTY min 1 time 0 || exit 1\n\
        byte=$($DD bs=1 count=1 2>/dev/null | $OD -An -tx1)\n\
        $STTY min 0 time 1\n\
        while :; do\n\
          n=$($DD bs=4096 count=1 2>/dev/null | $WC -c)\n\
          case \"$n\" in *[1-9]*) ;; *) break;; esac\n\
        done\n\
        echo \"$byte\"\n";

    /// One keypress from the terminal, without enter. Input typed ahead is thrown away first;
    /// after the key, whatever else arrives in the next 100 ms (the rest of an escape sequence,
    /// autorepeat, an Enter pressed from habit) is thrown away too. `None` when it cannot be
    /// done (no `sh`, `stty`, `dd`, `od` or `wc`, or the script failed): the caller then reads a line.
    pub fn read_key() -> Option<Key> {
        let (s, d, o, w) = (tool("stty")?, tool("dd")?, tool("od")?, tool("wc")?);
        let out = sh(KEY, &[&s, &d, &o, &w])?
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
        Some(key_of(byte))
    }
}

#[cfg(unix)]
pub use unix::{drain_input, query_background, read_key, tool};

#[cfg(not(unix))]
pub fn query_background(_timeout: Duration) -> Option<Rgb> {
    None
}

#[cfg(not(unix))]
pub fn read_key() -> Option<Key> {
    None
}

#[cfg(not(unix))]
pub fn drain_input() -> bool {
    false
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

    #[test]
    fn a_byte_that_starts_an_escape_sequence_is_not_an_answer() {
        assert_eq!(key_of(0x1b), Key::Ignored, "an arrow key starts with ESC");
        assert_eq!(key_of(0x00), Key::Ignored);
        assert_eq!(key_of(0x03), Key::Ignored);
        assert_eq!(key_of(0x04), Key::Eof);
        assert_eq!(key_of(b'\n'), Key::Enter);
        assert_eq!(key_of(b'\r'), Key::Enter);
        assert_eq!(key_of(b'y'), Key::Char('y'));
        assert_eq!(key_of(b' '), Key::Ignored);
        assert_eq!(key_of(0xc3), Key::Ignored, "only ASCII keys count");
    }

    #[test]
    fn foreground_means_the_terminal_foreground_group_is_ours() {
        let fg = "123 (cigi) S 100 123 100 34816 123 4194304 1 0 0";
        assert_eq!(foreground_in_stat(fg), Some(true));
        let bg = "123 (cigi) S 100 123 100 34816 999 4194304 1 0 0";
        assert_eq!(foreground_in_stat(bg), Some(false));
        let none = "123 (cigi) S 100 123 100 0 -1 4194304 1 0 0";
        assert_eq!(foreground_in_stat(none), Some(false), "no terminal");
        let odd = "123 (a b) c) S 100 123 100 34816 123 4194304";
        assert_eq!(
            foreground_in_stat(odd),
            Some(true),
            "parens in the command name"
        );
        assert_eq!(foreground_in_stat("garbage"), None);
    }

    #[test]
    fn a_saved_terminal_state_is_one_safe_word() {
        assert!(safe_stty_state("4500:5:bf:8a3b:3:1c:7f:15"));
        assert!(safe_stty_state("gfmt1:cflag=4b00:iflag=2b02"));
        assert!(!safe_stty_state("-F"), "a leading dash would be an option");
        assert!(!safe_stty_state("--help"));
        assert!(!safe_stty_state(""));
        assert!(!safe_stty_state("a b"));
        assert!(!safe_stty_state("a;rm"));
        assert!(!safe_stty_state("$(x)"));
    }

    #[cfg(unix)]
    #[test]
    fn helpers_are_found_by_absolute_path_only() {
        for name in ["sh", "stty", "dd", "od", "wc"] {
            let p = tool(name).unwrap_or_else(|| panic!("{name} is on every unix"));
            assert!(p.is_absolute(), "{name}: {p:?}");
            assert!(p.starts_with("/bin") || p.starts_with("/usr/bin"), "{p:?}");
        }
        assert!(tool("definitely-not-a-tool").is_none());
    }
}
