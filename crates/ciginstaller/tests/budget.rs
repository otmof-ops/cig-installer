// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `first_line_budgets` (#32): `ciginstaller` (the status) writes its first line within 50 ms
//! and any other command within 100 ms, measured from spawning the process to its first byte
//! on a pipe, on the debug build a test gets. Machines are noisy, so the best of seven runs
//! counts, and the number is printed. There is no terminal here, so the background query
//! (also bounded at 50 ms) is not part of this number; `cigi-ui`'s own tests bound it.

mod common;

use std::io::Read;
use std::process::Stdio;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use cigi_testkit::Fixture;
use common::{cigi, fixture, ok};

fn first_byte(fx: &Fixture, args: &[&str]) -> Duration {
    let start = Instant::now();
    let mut child = fx
        .cigi()
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run ciginstaller");
    let (tx, rx) = mpsc::channel();
    let out = child.stdout.take().map(|o| (o, tx.clone()));
    let err = child.stderr.take().map(|e| (e, tx));
    let mut readers = Vec::new();
    if let Some((mut o, tx)) = out {
        readers.push(std::thread::spawn(move || {
            let mut b = [0u8; 1];
            if o.read(&mut b).unwrap_or(0) > 0 {
                let _ = tx.send(Instant::now());
            }
            let _ = std::io::copy(&mut o, &mut std::io::sink());
        }));
    }
    if let Some((mut e, tx)) = err {
        readers.push(std::thread::spawn(move || {
            let mut b = [0u8; 1];
            if e.read(&mut b).unwrap_or(0) > 0 {
                let _ = tx.send(Instant::now());
            }
            let _ = std::io::copy(&mut e, &mut std::io::sink());
        }));
    }
    let first = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("a first byte");
    let _ = child.wait();
    for r in readers {
        let _ = r.join();
    }
    first.duration_since(start)
}

fn best_of(fx: &Fixture, args: &[&str]) -> Duration {
    (0..7)
        .map(|_| first_byte(fx, args))
        .min()
        .unwrap_or(Duration::MAX)
}

#[test]
fn first_line_budgets() {
    let mut fx = fixture();
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.16.0")]);
    fx.publish("stable");
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));

    let status = best_of(&fx, &[]);
    let list = best_of(&fx, &["list"]);
    let explain = best_of(&fx, &["explain", "E1811"]);
    let error = best_of(&fx, &["carton", "9.9"]);
    eprintln!("first line: status {status:?}, list {list:?}, explain {explain:?}, error {error:?}");

    assert!(
        status <= Duration::from_millis(cigi_ui::tokens::budget::STATUS_FIRST_LINE_MS),
        "the status's first line took {status:?}, budget 50 ms"
    );
    for (what, took) in [("list", list), ("explain", explain), ("an error", error)] {
        assert!(
            took <= Duration::from_millis(cigi_ui::tokens::budget::FIRST_LINE_MS),
            "{what}: first line took {took:?}, budget 100 ms"
        );
    }
}
