// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! The parts that touch a real terminal, proved on a pseudo-terminal made by `script(1)`
//! (util-linux), the way the installer's own update tests do. The `probe` example asks two
//! questions in a row (one keypress each); the test plays the terminal and the typist.
//!
//! * typed-ahead input never answers a question;
//! * an arrow key (an ESC sequence) is not an answer;
//! * an Enter pressed after a key does not answer the next question;
//! * a late OSC 11 reply never answers a question;
//! * a hostile `PATH` is never used to find `stty`, `dd` or `sh`;
//! * three typed answers that are not options end the run with exit 3.

#![cfg(target_os = "linux")]

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn have_script() -> bool {
    Command::new("script")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// The `probe` example, built fresh: a test target asked for alone does not build examples.
fn probe() -> PathBuf {
    let exe = std::env::current_exe().expect("test exe");
    let debug = exe.parent().and_then(Path::parent).expect("target dir");
    let p = debug.join("examples").join("probe");
    // always: a stale example from an earlier build would answer for the current code
    {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut cmd = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
        cmd.current_dir(root)
            .args(["build", "--example", "probe", "--offline"]);
        if let Some(target) = debug.parent() {
            cmd.arg("--target-dir").arg(target);
        }
        assert!(
            cmd.status().expect("run cargo").success(),
            "build the probe example"
        );
    }
    p
}

struct Tty {
    child: Child,
    stdin: ChildStdin,
    out: Arc<Mutex<Vec<u8>>>,
}

impl Tty {
    /// Run the probe on a pseudo-terminal with a clean environment plus `env`.
    fn start(env: &[(&str, &str)]) -> Tty {
        let mut c = Command::new("script");
        c.env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("SHELL", "/bin/sh")
            .env("TERM", "xterm")
            .env("LANG", "en_US.UTF-8")
            .env("COLORFGBG", "15;0")
            .envs(env.iter().copied())
            .args(["-qec", &probe().to_string_lossy(), "/dev/null"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = c.spawn().expect("run script(1)");
        let stdin = child.stdin.take().expect("stdin");
        let mut stdout = child.stdout.take().expect("stdout");
        let out = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&out);
        std::thread::spawn(move || {
            let mut b = [0u8; 512];
            while let Ok(n) = stdout.read(&mut b) {
                if n == 0 {
                    break;
                }
                if let Ok(mut o) = sink.lock() {
                    o.extend_from_slice(&b[..n]);
                }
            }
        });
        Tty { child, stdin, out }
    }

    fn send(&mut self, s: &str) {
        let _ = self.stdin.write_all(s.as_bytes());
        let _ = self.stdin.flush();
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.out.lock().map(|o| o.clone()).unwrap_or_default())
            .into_owned()
    }

    fn wait_for(&self, needle: &str) -> bool {
        let end = Instant::now() + Duration::from_secs(6);
        while Instant::now() < end {
            if self.text().contains(needle) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    /// Play the terminal: answer the background query with `rgb` after `delay`.
    fn answer_background(&mut self, rgb: &str, delay: Duration) {
        if self.wait_for("\x1b]11;?") {
            std::thread::sleep(delay);
            self.send(&format!("\x1b]11;rgb:{rgb}\x07"));
        }
    }

    fn finish(mut self) -> (Option<i32>, String) {
        let end = Instant::now() + Duration::from_secs(6);
        loop {
            if let Ok(Some(st)) = self.child.try_wait() {
                std::thread::sleep(Duration::from_millis(50));
                return (st.code(), self.text());
            }
            if Instant::now() >= end {
                let _ = self.child.kill();
                return (None, self.text());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

fn pause(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

#[test]
fn a_key_answers_with_no_enter() {
    if !have_script() {
        return;
    }
    let mut t = Tty::start(&[]);
    assert!(t.wait_for("press a key (1)"), "{}", t.text());
    pause(300);
    t.send("n");
    assert!(t.wait_for("answer 1: 'n' by Typed"), "{}", t.text());
    assert!(t.wait_for("press a key (2)"));
    pause(300);
    t.send("y");
    let (code, out) = t.finish();
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("answer 2: 'y' by Typed"), "{out}");
}

#[test]
fn typed_ahead_input_never_answers_a_question() {
    if !have_script() {
        return;
    }
    let mut t = Tty::start(&[]);
    t.send("y"); // before the question is even drawn
    pause(250);
    assert!(t.wait_for("press a key (1)"), "{}", t.text());
    pause(300);
    t.send("n");
    assert!(
        t.wait_for("answer 1: 'n' by Typed"),
        "the typed-ahead y was thrown away: {}",
        t.text()
    );
}

#[test]
fn an_arrow_key_is_not_an_answer_and_is_not_counted_as_a_miss() {
    if !have_script() {
        return;
    }
    let mut t = Tty::start(&[]);
    assert!(t.wait_for("press a key (1)"));
    pause(300);
    t.send("\x1b[A\x1b[B"); // up, down
    pause(400);
    assert!(
        !t.text().contains("answer 1"),
        "an arrow answered: {}",
        t.text()
    );
    t.send("y");
    assert!(t.wait_for("answer 1: 'y' by Typed"), "{}", t.text());
    assert!(!t.text().contains("pick one of"), "{}", t.text());
}

#[test]
fn an_enter_after_the_key_does_not_answer_the_next_question() {
    if !have_script() {
        return;
    }
    let mut t = Tty::start(&[]);
    assert!(t.wait_for("press a key (1)"));
    pause(300);
    t.send("y\r\r"); // the habit: key, then enter
    assert!(t.wait_for("answer 1: 'y' by Typed"), "{}", t.text());
    assert!(t.wait_for("press a key (2)"));
    pause(500);
    assert!(
        !t.text().contains("answer 2"),
        "the Enter answered the second question: {}",
        t.text()
    );
    t.send("n");
    assert!(t.wait_for("answer 2: 'n' by Typed"), "{}", t.text());
}

#[test]
fn a_late_background_reply_never_answers_a_question() {
    if !have_script() {
        return;
    }
    // no COLORFGBG, so the installer asks; the terminal answers 150 ms late
    let mut t = Tty::start(&[("COLORFGBG", "")]);
    t.answer_background("ffff/ffff/ffff", Duration::from_millis(150));
    assert!(t.wait_for("press a key (1)"), "{}", t.text());
    pause(400);
    assert!(
        !t.text().contains("answer 1"),
        "the reply's bytes answered: {}",
        t.text()
    );
    t.send("y");
    assert!(t.wait_for("answer 1: 'y' by Typed"), "{}", t.text());
}

#[test]
fn an_in_time_background_reply_picks_the_theme() {
    if !have_script() {
        return;
    }
    let mut t = Tty::start(&[("COLORFGBG", "")]);
    t.answer_background("ffff/ffff/ffff", Duration::from_millis(0));
    assert!(t.wait_for("press a key (1)"), "{}", t.text());
    assert!(t.text().contains("theme Light"), "{}", t.text());
}

#[test]
fn a_silent_terminal_gets_the_neutral_theme_within_the_budget() {
    if !have_script() {
        return;
    }
    let t = Tty::start(&[("COLORFGBG", "")]);
    assert!(t.wait_for("press a key (1)"), "{}", t.text());
    let text = t.text();
    assert!(text.contains("theme Neutral"), "{text}");
    // `found in N ms`: the query is bounded at 50 ms (plus the process starts around it)
    let ms: u64 = text
        .split("found in ")
        .nth(1)
        .and_then(|r| r.split(' ').next())
        .and_then(|n| n.parse().ok())
        .expect("a time");
    assert!(ms < 150, "the query took {ms} ms");
}

#[test]
fn a_hostile_path_is_never_used_to_find_the_helpers() {
    if !have_script() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let marker = dir.path().join("RAN");
    for name in ["stty", "dd", "sh", "od", "wc", "kill"] {
        let p = dir.path().join(name);
        std::fs::write(&p, format!("#!/bin/sh\ntouch '{}'\n", marker.display())).expect("write");
        let _ = Command::new("chmod").arg("755").arg(&p).status();
    }
    let path = format!("{}:/usr/bin:/bin", dir.path().display());
    // both the background query and the keypress run their helpers
    let mut t = Tty::start(&[("PATH", &path), ("COLORFGBG", "")]);
    t.answer_background("0000/0000/0000", Duration::from_millis(0));
    assert!(t.wait_for("press a key (1)"), "{}", t.text());
    pause(300);
    t.send("y");
    assert!(t.wait_for("answer 1: 'y' by Typed"), "{}", t.text());
    assert!(!marker.exists(), "a program on PATH was run");
}

#[test]
fn three_answers_that_are_not_options_end_the_run_with_exit_3() {
    if !have_script() {
        return;
    }
    // plain: typed lines
    let mut t = Tty::start(&[("CIG_PLAIN", "1")]);
    assert!(t.wait_for("press a key (1)"), "{}", t.text());
    pause(300);
    for _ in 0..3 {
        t.send("maybe\n");
        pause(150);
    }
    let (code, out) = t.finish();
    assert_eq!(code, Some(3), "{out}");
    assert!(out.contains("pick one of: y n"), "{out}");
    assert!(out.contains("nothing was changed"), "{out}");
    assert!(!out.contains("answer 1"), "no default was taken: {out}");
}

#[test]
fn plain_answers_are_typed_lines() {
    if !have_script() {
        return;
    }
    let mut t = Tty::start(&[("CI", "1")]);
    assert!(t.wait_for("press a key (1)"), "{}", t.text());
    pause(300);
    t.send("no\n");
    assert!(t.wait_for("answer 1: 'n' by Typed"), "{}", t.text());
}
