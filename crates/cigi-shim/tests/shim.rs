// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0

//! Runs the built shim against a fabricated home: routing, the slow path, and spike S2.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn scratch(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn script(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// A home with two cartons; each fake tool prints who it is and what it was given.
fn home(root: &Path) -> PathBuf {
    let h = root.join("home");
    let tool = |t: &str, v: &str| {
        let p = h.join(format!("toolchains/{t}/{v}/bin/{t}"));
        script(
            &p,
            &format!("echo \"{t} {v} carton=$CIGI_CARTON managed=$CIGI_MANAGED argv0=$0 args=$*\""),
        );
        p
    };
    let (cig1, cig2, cigo1) = (
        tool("cig", "1.20.0"),
        tool("cig", "1.21.0"),
        tool("cigo", "1.17.0"),
    );
    let index = format!(
        "cigi-index 1\nmachine\t0.1.0\ncarton\t0.1.0\tcig\t{}\ncarton\t0.1.0\tcigo\t{}\ncarton\t0.2.0\tcig\t{}\n\
         carton-minor\t0.1\t0.1.0\ncarton-minor\t0.2\t0.2.0\nlegacy-pin-files\t.cig-version\t.cigar-version\n",
        cig1.display(), cigo1.display(), cig2.display()
    );
    fs::create_dir_all(h.join("installer")).unwrap();
    fs::write(h.join("installer/shim-index"), index).unwrap();
    script(&h.join("bin/ciginstaller"), "echo \"slow-path $*\"");
    h
}

fn shim_as(root: &Path, name: &str) -> PathBuf {
    let dir = root.join("shims");
    fs::create_dir_all(&dir).unwrap();
    let dest = dir.join(name);
    if !dest.exists() {
        fs::copy(env!("CARGO_BIN_EXE_cigi-shim"), &dest).unwrap();
    }
    dest
}

/// `Command::output`, tried again while exec fails with "text file busy": a
/// binary copied a moment ago can still be open in a child another test
/// thread forked, until that child execs.
fn output(c: &mut Command) -> Output {
    let mut tries = 0;
    loop {
        match c.output() {
            Err(e) if e.raw_os_error() == Some(26) && tries < 100 => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            r => return r.unwrap(),
        }
    }
}

fn run(root: &Path, h: &Path, name: &str, cwd: &Path, args: &[&str]) -> Output {
    output(
        Command::new(shim_as(root, name))
            .args(args)
            .current_dir(cwd)
            .env_remove("CIGI_CARTON")
            .env("CIGSCRIPT_HOME", h),
    )
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

#[test]
fn routes_by_pin_machine_and_env() {
    let root = scratch("routes");
    let h = home(&root);
    let (a, b, c) = (root.join("a"), root.join("b"), root.join("c/deep/er"));
    for d in [&a, &b, &c] {
        fs::create_dir_all(d).unwrap();
    }
    fs::write(a.join(".cig-carton"), "carton 0.1\n").unwrap();
    fs::write(
        b.join("cig-tasks.json"),
        r#"{"carton": "0.2", "tasks": {}}"#,
    )
    .unwrap();
    fs::write(root.join("c/cigar.toml"), "carton = \"0.2\"\n[x]\n").unwrap();

    let o = run(&root, &h, "cig", &a, &["--version"]);
    assert_eq!(
        out(&o),
        format!(
            "cig 1.20.0 carton=0.1.0 managed=1 argv0={} args=--version",
            out(&o)
                .split("argv0=")
                .nth(1)
                .unwrap()
                .split(' ')
                .next()
                .unwrap()
        )
    );
    assert!(out(&o).contains("argv0=") && out(&o).contains("cig/1.20.0/bin/cig"));
    assert!(out(&run(&root, &h, "cig", &b, &[])).starts_with("cig 1.21.0 carton=0.2.0 managed=1"));
    assert!(
        out(&run(&root, &h, "cig", &c, &[])).starts_with("cig 1.21.0 carton=0.2.0"),
        "found 3 levels up"
    );
    assert!(out(&run(&root, &h, "cigo", &root, &["x", "y"])).ends_with("args=x y"));
    assert!(
        out(&run(&root, &h, "cig", &root, &[])).starts_with("cig 1.20.0 carton=0.1.0"),
        "machine carton"
    );

    let o = output(
        Command::new(shim_as(&root, "cig"))
            .current_dir(&b)
            .env("CIGSCRIPT_HOME", &h)
            .env("CIGI_CARTON", "0.1.0"),
    );
    assert!(
        out(&o).starts_with("cig 1.20.0 carton=0.1.0"),
        "CIGI_CARTON beats the project pin"
    );
}

#[test]
fn slow_path_cases() {
    let root = scratch("slow");
    let h = home(&root);
    let p = root.join("p");
    fs::create_dir_all(&p).unwrap();
    fs::write(p.join(".cig-version"), ">=1.6.2\n").unwrap();
    let o = run(&root, &h, "cig", &p, &["run", "a.cig"]);
    assert!(
        out(&o).starts_with("slow-path __resolve --tool cig --cwd "),
        "{}",
        out(&o)
    );
    assert!(out(&o).ends_with("-- run a.cig"));
    fs::write(p.join(".cig-carton"), "carton >=0.1.2\n").unwrap();
    assert!(out(&run(&root, &h, "cig", &p, &[])).starts_with("slow-path"));
    // cigo is not in carton 0.2: slow path, which explains
    fs::write(p.join(".cig-carton"), "0.2\n").unwrap();
    assert!(out(&run(&root, &h, "cigo", &p, &[])).starts_with("slow-path"));
    // no ciginstaller and no index: E1840, exit 127
    fs::remove_file(h.join("bin/ciginstaller")).unwrap();
    let o = run(&root, &h, "cigar", &p, &[]);
    assert_eq!(o.status.code(), Some(127));
    assert!(String::from_utf8_lossy(&o.stderr).contains("error[E1840 installer]"));
}

#[test]
fn misnamed_and_looping_shims_exit_127_with_e1844() {
    let root = scratch("misnamed");
    let h = home(&root);
    let o = run(&root, &h, "cargo-thing", &root, &[]);
    assert_eq!(o.status.code(), Some(127));
    assert!(String::from_utf8_lossy(&o.stderr).contains("E1844"));
    let looping = format!(
        "cigi-index 1\nmachine\t0.1.0\ncarton\t0.1.0\tcig\t{}/bin/cig\n",
        h.display()
    );
    fs::write(h.join("installer/shim-index"), looping).unwrap();
    let o = run(&root, &h, "cig", &root, &[]);
    assert_eq!(o.status.code(), Some(127));
    assert!(String::from_utf8_lossy(&o.stderr).contains("E1844"));
}

#[test]
fn child_exit_code_passes_through() {
    let root = scratch("exitcode");
    let h = home(&root);
    script(&h.join("toolchains/cig/1.20.0/bin/cig"), "exit 42");
    assert_eq!(run(&root, &h, "cig", &root, &[]).status.code(), Some(42));
}

/// Spike S2: a 2 KB index, a 40-deep working directory, exec of `/bin/true`.
/// Prints the median of 1000 spawns for SPIKES.md. The assertion is on the shim's own
/// clock (process start to just before exec), which is what the contract budgets.
#[test]
fn spike_s2_median_under_one_millisecond() {
    let root = scratch("s2");
    let h = root.join("home");
    fs::create_dir_all(h.join("installer")).unwrap();
    let mut index = String::from("cigi-index 1\nmachine\t0.1.0\ncarton\t0.1.0\tcig\t/bin/true\n");
    for i in 0..12 {
        index.push_str(&format!(
            "carton\t0.{i}.9\tcigo\t/home/u/.cigscript/toolchains/cigo/1.{i}.0/bin/cigo\n"
        ));
    }
    index.push_str("carton-minor\t0.1\t0.1.0\nlegacy-pin-files\t.cig-version\t.cigar-version\n");
    while index.len() < 2048 {
        index.push_str("stick\tcigar\t/home/u/.cigscript/toolchains/cigar/0.1.0/bin/cigar\n");
    }
    fs::write(h.join("installer/shim-index"), &index).unwrap();
    let mut deep = root.join("project");
    for i in 0..40 {
        deep.push(format!("d{i}"));
    }
    fs::create_dir_all(&deep).unwrap();
    fs::write(root.join("project/.cig-carton"), "carton 0.1\n").unwrap(); // found only at the top: the whole walk is paid
    let shim = shim_as(&root, "cig");

    let (mut wall, mut own) = (Vec::new(), Vec::new());
    for _ in 0..1000 {
        let t = std::time::Instant::now();
        let o = output(
            Command::new(&shim)
                .current_dir(&deep)
                .env("CIGSCRIPT_HOME", &h)
                .env("CIGI_SHIM_TIMING", "1")
                .env_remove("CIGI_CARTON"),
        );
        wall.push(t.elapsed().as_nanos());
        assert!(o.status.success());
        let err = String::from_utf8_lossy(&o.stderr);
        own.push(
            err.trim()
                .strip_prefix("cigi-shim-timing-ns: ")
                .unwrap()
                .parse::<u128>()
                .unwrap(),
        );
    }
    let med = |v: &mut Vec<u128>| {
        v.sort_unstable();
        v[v.len() / 2]
    };
    let mut base = Vec::new();
    for _ in 0..1000 {
        let t = std::time::Instant::now();
        let _ = Command::new("/bin/true").output().unwrap();
        base.push(t.elapsed().as_nanos());
    }
    let (w, o, b) = (med(&mut wall), med(&mut own), med(&mut base));
    eprintln!("S2 index={} bytes, depth=40: shim-own median {o} ns, spawn+exec+wait median {w} ns, /bin/true alone {b} ns", index.len());
    assert!(o < 1_000_000, "shim median {o} ns is over the 1 ms budget");
}
