// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! Shared helpers for the acceptance tests: a fixture whose installer can find its shim.
#![allow(dead_code)]

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::Once;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cigi_testkit::Fixture;

static BUILD_SHIM: Once = Once::new();

/// `cigi-shim` is a binary of another package, so cargo does not build it for these tests.
/// Build it once beside the installer (the installer looks for it next to itself).
pub fn ensure_shim() {
    BUILD_SHIM.call_once(|| {
        let exe = std::env::current_exe().unwrap_or_default();
        let profile_dir = exe
            .parent()
            .and_then(|d| d.parent())
            .map(PathBuf::from)
            .unwrap_or_default();
        let shim = profile_dir.join(format!("cigi-shim{}", std::env::consts::EXE_SUFFIX));
        if shim.is_file() {
            return;
        }
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut cmd = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
        cmd.current_dir(root)
            .args(["build", "-p", "cigi-shim", "--offline"]);
        if profile_dir.file_name().is_some_and(|n| n == "release") {
            cmd.arg("--release");
        }
        if let Some(target) = profile_dir.parent() {
            cmd.arg("--target-dir").arg(target);
        }
        let st = cmd.status().expect("run cargo to build cigi-shim");
        assert!(st.success(), "building cigi-shim failed");
    });
}

pub fn fixture() -> Fixture {
    ensure_shim();
    Fixture::new()
}

pub fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

/// Run the installer with `--yes` and return the output; panics when it cannot start.
pub fn cigi(fx: &Fixture, args: &[&str]) -> Output {
    fx.cigi()
        // The fixture's releases folder serves the manifest, the checksums and the files
        // itself: one source, accepted only with the opt-in (`a2_single_source_refused`
        // proves the default).
        .env("CIGI_ALLOW_SINGLE_SOURCE", "true")
        .arg("--yes")
        .args(args)
        .output()
        .expect("run ciginstaller")
}

pub fn ok(o: &Output) {
    assert_eq!(o.status.code(), Some(0), "{}", text(o));
}

pub fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

/// Run a command (built to start `script(1)`, so the child has a terminal) and type at its
/// prompts: chunk `i` is sent once the `i`th question has been drawn (each shows `(enter =`)
/// and a moment (`TYPE_PAUSE`) has passed. Returns the exit code and everything printed.
///
/// A question throws away what was typed before it was ready to read: after drawing itself it
/// starts `sh`, `stty` and `dd` to drain the terminal (`cigi_ui::detect::read_key` and
/// `drain_input`), and only then reads. How long that takes is the machine's: on a loaded one it
/// can outlast any fixed pause, the key is drained, and the question waits for a key that is not
/// coming. So a chunk that has not been taken is typed again: while nothing but its own echo has
/// been printed since it was typed, the next question is not drawn, and the output has been quiet
/// for `RETYPE_AFTER`, the same chunk goes again. Once the child prints anything else, the chunk
/// was taken and is not typed again. A chunk typed again before the next question is drawn is
/// drained by that question in turn, so it can never answer it.
///
/// No test may wait forever on a prompt: after [`pty_timeout`] the child is killed and the test
/// fails with everything it printed and the chunk it was waiting on. Every run prints one line
/// (shown with `--nocapture` or on a failure) with how long it took and how often it typed.
///
/// The longest a terminal test may run on an ordinary machine.
pub const PTY_TIMEOUT: Duration = Duration::from_secs(30);

/// `PTY_TIMEOUT` times `CIGI_TEST_PTY_SCALE` (a whole number from 1 to 20; unset is 1), for a
/// machine that is slow or busy: most of a terminal test's time is the debug build's install
/// after the last answer, and that scales with the machine's load (`docs/BENCH.md`).
pub fn pty_timeout() -> Duration {
    match std::env::var("CIGI_TEST_PTY_SCALE") {
        Err(_) => PTY_TIMEOUT,
        Ok(v) => match v.trim().parse::<u32>() {
            Ok(n @ 1..=20) => PTY_TIMEOUT * n,
            _ => panic!("CIGI_TEST_PTY_SCALE is `{v}`: a whole number from 1 to 20, or unset"),
        },
    }
}
/// The pause between a question being drawn and its chunk being typed.
const TYPE_PAUSE: Duration = Duration::from_millis(350);
/// How long a chunk may go with nothing printed and the next question not drawn before it is
/// typed again.
const RETYPE_AFTER: Duration = Duration::from_millis(1000);

pub fn type_at_prompts(mut cmd: Command, chunks: &[&str]) -> (i32, String) {
    let started = Instant::now();
    let cap = pty_timeout();
    let deadline = started + cap;
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("run script(1)");
    let mut input = child.stdin.take().expect("stdin");
    let seen = Arc::new(Mutex::new(Vec::<u8>::new()));
    let mut readers = Vec::new();
    for stream in [
        child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
        child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    ]
    .into_iter()
    .flatten()
    {
        let sink = Arc::clone(&seen);
        readers.push(std::thread::spawn(move || {
            let mut stream = stream;
            let mut b = [0u8; 1024];
            while let Ok(n) = stream.read(&mut b) {
                if n == 0 {
                    break;
                }
                if let Ok(mut s) = sink.lock() {
                    s.extend_from_slice(&b[..n]);
                }
            }
        }));
    }
    // (questions drawn, bytes printed)
    let progress = |seen: &Arc<Mutex<Vec<u8>>>| {
        let s = seen.lock().map(|s| s.clone()).unwrap_or_default();
        let drawn = String::from_utf8_lossy(&s).matches("(enter =").count();
        (drawn, s.len())
    };
    // whether the child printed anything but the echo of `chunk` after byte `from`
    let answered = |seen: &Arc<Mutex<Vec<u8>>>, from: usize, chunk: &str| {
        let s = seen.lock().map(|s| s.clone()).unwrap_or_default();
        let after = String::from_utf8_lossy(s.get(from..).unwrap_or_default()).into_owned();
        after
            .chars()
            .any(|c| !c.is_whitespace() && !chunk.contains(c))
    };
    let typed = |input: &mut std::process::ChildStdin, chunk: &str| {
        let _ = input.write_all(chunk.as_bytes());
        let _ = input.flush();
    };
    // what the harness was doing, for the message when the cap is hit
    let mut waiting_on = String::from("the child to start");
    let mut timed_out = false;
    let mut exited = None;
    let mut typed_all = 0;
    'chunks: for (i, chunk) in chunks.iter().enumerate() {
        waiting_on = format!("question {} to be drawn", i + 1);
        loop {
            if let Some(st) = child.try_wait().ok().flatten() {
                exited = Some(st);
                break 'chunks;
            }
            if progress(&seen).0 > i {
                break;
            }
            if Instant::now() >= deadline {
                timed_out = true;
                break 'chunks;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(TYPE_PAUSE);
        let from = progress(&seen).1;
        typed(&mut input, chunk);
        typed_all += 1;
        let last = i + 1 == chunks.len();
        let mut tries = 1;
        let mut taken = false;
        let mut since = (Instant::now(), from);
        let then = if last { " or the child to exit" } else { "" };
        let say = |tries: usize| {
            format!(
                "chunk {} ({chunk:?}) to be taken{then}: typed {tries} time(s), the last {:.1} s in",
                i + 1,
                started.elapsed().as_secs_f64()
            )
        };
        waiting_on = say(tries);
        loop {
            if let Some(st) = child.try_wait().ok().flatten() {
                exited = Some(st);
                break 'chunks;
            }
            let (drawn, printed) = progress(&seen);
            if !last && drawn > i + 1 {
                break;
            }
            if Instant::now() >= deadline {
                timed_out = true;
                break 'chunks;
            }
            if !taken && printed != from && answered(&seen, from, chunk) {
                taken = true;
                waiting_on =
                    format!(
                    "{} after chunk {} ({chunk:?}) was taken, typed {tries} time(s), {:.1} s in",
                    if last { "the child to exit" } else { "the next question" },
                    i + 1,
                    started.elapsed().as_secs_f64()
                );
            }
            if printed != since.1 {
                since = (Instant::now(), printed);
            } else if !taken && since.0.elapsed() >= RETYPE_AFTER {
                typed(&mut input, chunk);
                typed_all += 1;
                tries += 1;
                waiting_on = say(tries);
                since = (Instant::now(), printed);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    // script ends the child at EOF, so the input stays open until the child is done
    let status = match exited {
        Some(st) => st,
        None => loop {
            if timed_out {
                let _ = child.kill(); // the pty closes, so the child under it gets SIGHUP
                break child.wait().expect("reap script(1)");
            }
            if chunks.is_empty() {
                waiting_on = "the child to exit".into();
            }
            match child.try_wait().expect("wait for script(1)") {
                Some(st) => break st,
                None if Instant::now() >= deadline => timed_out = true,
                None => std::thread::sleep(Duration::from_millis(10)),
            }
        },
    };
    std::mem::forget(input);
    for r in readers {
        let _ = r.join();
    }
    let text =
        String::from_utf8_lossy(&seen.lock().map(|s| s.clone()).unwrap_or_default()).into_owned();
    eprintln!(
        "terminal test: {:.1} s, {} chunk(s), typed {typed_all} time(s) in all; last waited on {waiting_on}",
        started.elapsed().as_secs_f64(),
        chunks.len()
    );
    assert!(
        !timed_out,
        "the terminal test was still waiting after {cap:?} and was killed (CIGI_TEST_PTY_SCALE scales the cap for a slow machine); it was waiting on {waiting_on}; it printed:\n{text}"
    );
    (status.code().unwrap_or(-1), text)
}
