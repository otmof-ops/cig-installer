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
/// and a moment has passed. A question throws away what was typed before it was drawn, so
/// keys cannot be written ahead of time. Returns the exit code and everything printed.
///
/// No test may wait forever on a prompt: after `PTY_TIMEOUT` the child is killed and the test
/// fails with everything it printed.
/// The longest a terminal test may run.
pub const PTY_TIMEOUT: Duration = Duration::from_secs(30);

pub fn type_at_prompts(mut cmd: Command, chunks: &[&str]) -> (i32, String) {
    let deadline = Instant::now() + PTY_TIMEOUT;
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
    let drawn = |seen: &Arc<Mutex<Vec<u8>>>| {
        let s = seen.lock().map(|s| s.clone()).unwrap_or_default();
        String::from_utf8_lossy(&s).matches("(enter =").count()
    };
    for (i, chunk) in chunks.iter().enumerate() {
        while drawn(&seen) <= i && Instant::now() < deadline {
            if child.try_wait().ok().flatten().is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(350));
        let _ = input.write_all(chunk.as_bytes());
        let _ = input.flush();
    }
    // script ends the child at EOF, so the input stays open until the child is done
    let mut timed_out = false;
    let status = loop {
        match child.try_wait().expect("wait for script(1)") {
            Some(st) => break st,
            None if Instant::now() >= deadline => {
                timed_out = true;
                let _ = child.kill(); // the pty closes, so the child under it gets SIGHUP
                break child.wait().expect("reap script(1)");
            }
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    };
    std::mem::forget(input);
    for r in readers {
        let _ = r.join();
    }
    let text =
        String::from_utf8_lossy(&seen.lock().map(|s| s.clone()).unwrap_or_default()).into_owned();
    assert!(
        !timed_out,
        "the terminal test was still waiting after {PTY_TIMEOUT:?} and was killed; it printed:\n{text}"
    );
    (status.code().unwrap_or(-1), text)
}
