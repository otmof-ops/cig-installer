// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! Phase 2 acceptance: pin files and project discovery. One reader (the shim's) decides which pin
//! file counts, for the shim and for `ciginstaller`; `pin` edits what it finds and nothing else.

// Unix only: pin-file tests need Unix symlinks, modes and owners (the shim's trust rules).
// The Windows runtime drafts add their own equivalents.
#![cfg(unix)]

mod common;

use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use cigi_testkit::Fixture;
use common::{cigi, fixture, ok, out, text};

fn cartons(fx: &mut Fixture) {
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.16.0")]);
    fx.carton("0.2.0", &[("cig", "1.21.0"), ("cigo", "1.17.0")]);
    fx.publish("stable");
}

/// Two cartons published, 0.1.0 installed first (the machine carton).
fn base() -> Fixture {
    let mut fx = fixture();
    cartons(&mut fx);
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    fx
}

/// A fresh directory inside the scratch HOME (so the walk stops there).
fn dir(fx: &Fixture, name: &str) -> PathBuf {
    let d = fx.user_home.join(name);
    fs::create_dir_all(&d).unwrap();
    d
}

fn pin(fx: &Fixture, cwd: &Path, args: &[&str]) -> Output {
    fx.cigi()
        .arg("--yes")
        .arg("pin")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("run ciginstaller pin")
}

fn which(fx: &Fixture, cwd: &Path, args: &[&str]) -> Output {
    run_timeout(
        fx.cigi().arg("which").args(args).current_dir(cwd),
        Duration::from_secs(20),
    )
}

/// Run to the end or fail the test: a hang is the bug these tests look for.
fn run_timeout(c: &mut Command, limit: Duration) -> Output {
    c.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut tries = 0;
    let mut child = loop {
        match c.spawn() {
            // text file busy: a file written a moment ago is still open in a forked child
            Err(e) if e.raw_os_error() == Some(26) && tries < 100 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(5));
            }
            r => break r.expect("spawn"),
        }
    };
    let t0 = Instant::now();
    loop {
        if child.try_wait().expect("wait").is_some() {
            return child.wait_with_output().expect("output");
        }
        if t0.elapsed() > limit {
            let _ = child.kill();
            let _ = child.wait();
            panic!("hung for more than {limit:?}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The shim, as the user's shell would run it, never against the real home.
fn shim(fx: &Fixture, tool: &str, cwd: &Path) -> Output {
    let bin = fx.cigscript().join("bin");
    let mut c = Command::new(bin.join(tool));
    c.env_clear()
        .env("HOME", &fx.user_home)
        .env("CIGSCRIPT_HOME", fx.cigscript())
        .env("CIGI_MIRROR", fx.mirror_file_url())
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .env("LANG", "C")
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .current_dir(cwd);
    run_timeout(&mut c, Duration::from_secs(30))
}

fn read(p: &Path) -> String {
    fs::read_to_string(p).unwrap()
}

// ----- pin writes -------------------------------------------------------------------------

#[test]
fn pin_carton_file_keeps_comments_crlf_and_mode() {
    let fx = base();
    let d = dir(&fx, "p");
    let f = d.join(".cig-carton");
    fs::write(&f, "# team pin, see wiki\r\ncarton 0.1\r\n# end\r\n").unwrap();
    fs::set_permissions(&f, fs::Permissions::from_mode(0o600)).unwrap();
    let o = pin(&fx, &d, &["carton", "0.2"]);
    ok(&o);
    assert_eq!(read(&f), "# team pin, see wiki\r\ncarton 0.2\r\n# end\r\n");
    assert_eq!(
        fs::metadata(&f).unwrap().permissions().mode() & 0o777,
        0o600
    );
    // a new file: 0644 under the umask, never wider
    let d2 = dir(&fx, "p2");
    ok(&pin(&fx, &d2, &["carton", "0.1"]));
    let f2 = d2.join(".cig-carton");
    assert_eq!(read(&f2), "carton 0.1\n");
    assert_eq!(
        fs::metadata(&f2).unwrap().permissions().mode() & !0o644,
        0o100000
    );
    // no temp file is left behind
    let names: Vec<_> = fs::read_dir(&d).unwrap().flatten().collect();
    assert_eq!(names.len(), 1, "{names:?}");
}

#[test]
fn pin_cig_tasks_json_is_a_targeted_edit() {
    let fx = base();
    let d = dir(&fx, "j");
    let f = d.join("cig-tasks.json");
    let old = "{\n  \"name\": \"x\",\n  \"tasks\": {\"carton\": \"9\", \"a\": [1, 2]},\n  \"carton\": \"0.1\"\n}\n";
    fs::write(&f, old).unwrap();
    ok(&pin(
        &fx,
        &d,
        &["carton", "0.2", "--file", "cig-tasks.json"],
    ));
    assert_eq!(
        read(&f),
        old.replace("\"carton\": \"0.1\"", "\"carton\": \"0.2\"")
    );
    // no key yet: inserted after the opening brace, CRLF kept
    let f2 = dir(&fx, "j2").join("cig-tasks.json");
    fs::write(&f2, "{\r\n  \"name\": \"x\"\r\n}\r\n").unwrap();
    ok(&pin(
        &fx,
        f2.parent().unwrap(),
        &["carton", "0.2", "--file", "cig-tasks.json"],
    ));
    assert_eq!(
        read(&f2),
        "{\r\n  \"carton\": \"0.2\",\r\n  \"name\": \"x\"\r\n}\r\n"
    );
    let v: serde_json::Value = serde_json::from_str(&read(&f2)).unwrap();
    assert_eq!(v["carton"], "0.2");
}

#[test]
fn pin_cigar_toml_keeps_arrays_comments_and_crlf() {
    let fx = base();
    let d = dir(&fx, "t");
    let f = d.join("cigar.toml");
    let old = "# project\nm = [\n  [1, 2],\n  [3, 4],\n]\nname = \"x\" # keep\n[deps]\nx = 1\n";
    fs::write(&f, old).unwrap();
    ok(&pin(&fx, &d, &["carton", "0.2", "--file", "cigar.toml"]));
    let new = read(&f);
    assert_eq!(
        new,
        old.replace(
            "name = \"x\" # keep\n",
            "name = \"x\" # keep\ncarton = \"0.2\"\n"
        )
    );
    let doc: toml_edit::DocumentMut = new.parse().unwrap();
    assert_eq!(doc["carton"].as_str(), Some("0.2"));
    assert!(doc["m"].as_array().is_some_and(|a| a.len() == 2));
    // an existing key keeps its trailing comment; CRLF stays
    let f2 = dir(&fx, "t2").join("cigar.toml");
    fs::write(
        &f2,
        "name = \"x\"\r\ncarton = \"0.1\" # the pin\r\n[d]\r\nx = 1\r\n",
    )
    .unwrap();
    ok(&pin(
        &fx,
        f2.parent().unwrap(),
        &["carton", "0.2", "--file", "cigar.toml"],
    ));
    assert_eq!(
        read(&f2),
        "name = \"x\"\r\ncarton = \"0.2\" # the pin\r\n[d]\r\nx = 1\r\n"
    );
}

#[test]
fn pin_leaves_a_file_it_cannot_read_untouched() {
    let fx = base();
    for (name, extra) in [
        (".cig-carton", &[][..]),
        ("cig-tasks.json", &["--file", "cig-tasks.json"][..]),
        ("cigar.toml", &["--file", "cigar.toml"][..]),
    ] {
        let d = dir(&fx, &format!("bad-{name}"));
        let f = d.join(name);
        let bytes: &[u8] = b"{\"name\": \"caf\xe9\"}\n";
        fs::write(&f, bytes).unwrap();
        let mut args = vec!["carton", "0.2"];
        args.extend_from_slice(extra);
        let o = pin(&fx, &d, &args);
        assert_ne!(o.status.code(), Some(0), "{name}: {}", text(&o));
        assert!(text(&o).contains("E1841"), "{name}: {}", text(&o));
        assert_eq!(fs::read(&f).unwrap(), bytes, "{name}");
    }
    // an unreadable file is not an empty one either
    let d = dir(&fx, "noperm");
    let f = d.join("cigar.toml");
    fs::write(&f, "name = \"x\"\n").unwrap();
    fs::set_permissions(&f, fs::Permissions::from_mode(0o000)).unwrap();
    let readable = fs::read(&f).is_ok(); // root reads anything
    let o = pin(&fx, &d, &["carton", "0.2", "--file", "cigar.toml"]);
    fs::set_permissions(&f, fs::Permissions::from_mode(0o644)).unwrap();
    if !readable {
        assert!(text(&o).contains("E1841"), "{}", text(&o));
        assert_eq!(read(&f), "name = \"x\"\n");
    }
}

#[test]
fn pin_refuses_a_symlinked_or_non_regular_target() {
    let fx = base();
    let victim = fx.user_home.join("victim");
    fs::write(&victim, "PRECIOUS").unwrap();
    for (name, extra) in [
        (".cig-carton", &[][..]),
        ("cig-tasks.json", &["--file", "cig-tasks.json"][..]),
        ("cigar.toml", &["--file", "cigar.toml"][..]),
    ] {
        let d = dir(&fx, &format!("link-{name}"));
        symlink(&victim, d.join(name)).unwrap();
        let mut args = vec!["carton", "0.2"];
        args.extend_from_slice(extra);
        let o = pin(&fx, &d, &args);
        assert_eq!(o.status.code(), Some(2), "{name}: {}", text(&o));
        assert!(text(&o).contains("E1846"), "{name}: {}", text(&o));
        assert!(fs::symlink_metadata(d.join(name))
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(read(&victim), "PRECIOUS");
    }
    // a directory in the way
    let d = dir(&fx, "isdir");
    fs::create_dir(d.join(".cig-carton")).unwrap();
    let o = pin(&fx, &d, &["carton", "0.2"]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
}

/// The attack the review reproduced: symlinks planted at the names the old writer used
/// (`.{label}.tmp-{pid}`, the label already dotted), for the pid of the very process that runs.
#[test]
fn a_planted_temp_symlink_is_never_followed() {
    use std::io::Write;
    let fx = base();
    let d = dir(&fx, "attack");
    let victim = fx.user_home.join("victim");
    fs::write(&victim, "PRECIOUS").unwrap();
    // `sh` waits on stdin, so its pid is known before it becomes ciginstaller (exec keeps the pid)
    let inst = cigi_testkit::binary_path("ciginstaller");
    let mut c = Command::new("sh");
    c.args(["-c", "read x; exec \"$0\" \"$@\"", inst.to_str().unwrap()])
        .args(["--yes", "pin", "carton", "0.2"])
        .env_clear()
        .current_dir(&d)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in fx.cigi().get_envs() {
        if let Some(v) = v {
            c.env(k, v);
        }
    }
    let mut child = c.spawn().unwrap();
    let pid = child.id();
    for n in [".cig-carton", "..cig-carton"] {
        symlink(&victim, d.join(format!("{n}.tmp-{pid}"))).unwrap();
    }
    child.stdin.take().unwrap().write_all(b"\n").unwrap();
    let o = child.wait_with_output().unwrap();
    ok(&o);
    assert_eq!(read(&victim), "PRECIOUS", "{}", text(&o));
    assert_eq!(read(&d.join(".cig-carton")), "carton 0.2\n");
    assert!(!fs::symlink_metadata(d.join(".cig-carton"))
        .unwrap()
        .file_type()
        .is_symlink());
}

#[test]
fn pin_refuses_when_a_higher_pin_file_would_outrank_it() {
    let fx = base();
    let d = dir(&fx, "rank");
    fs::write(d.join(".cig-carton"), "carton 0.1\n").unwrap();
    fs::write(d.join("cig-tasks.json"), "{\"a\": 1}\n").unwrap();
    for file in ["cig-tasks.json", "cigar.toml"] {
        let o = pin(&fx, &d, &["carton", "0.2", "--file", file]);
        assert_eq!(o.status.code(), Some(2), "{file}: {}", text(&o));
        assert!(text(&o).contains("E1847"), "{file}: {}", text(&o));
    }
    assert_eq!(read(&d.join("cig-tasks.json")), "{\"a\": 1}\n");
    assert!(!d.join("cigar.toml").exists());
    // cig-tasks.json outranks cigar.toml only when it pins
    let e = dir(&fx, "rank2");
    fs::write(e.join("cig-tasks.json"), "{\"a\": 1}\n").unwrap();
    ok(&pin(&fx, &e, &["carton", "0.2", "--file", "cigar.toml"]));
    fs::write(e.join("cig-tasks.json"), "{\"carton\": \"0.1\"}\n").unwrap();
    let o = pin(&fx, &e, &["carton", "0.2", "--file", "cigar.toml"]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    // and without --file the pin goes to .cig-carton, which outranks everything
    ok(&pin(&fx, &e, &["carton", "0.2"]));
}

#[test]
fn project_root_stops_at_home_and_trusts_only_plain_markers() {
    let fx = base();
    // a marker above HOME is not a project
    fs::create_dir(fx.tmp.path().join(".git")).unwrap();
    let deep = dir(&fx, "a/b");
    let o = pin(&fx, &deep, &["carton", "0.2"]);
    ok(&o);
    assert!(deep.join(".cig-carton").is_file(), "{}", text(&o));
    assert!(!fx.tmp.path().join(".cig-carton").exists());
    // a marker inside HOME is
    let top = fx.user_home.join("repo");
    fs::create_dir_all(top.join("sub/dir")).unwrap();
    fs::create_dir(top.join(".git")).unwrap();
    ok(&pin(&fx, &top.join("sub/dir"), &["carton", "0.2"]));
    assert!(top.join(".cig-carton").is_file());
    // a symlinked `.git` is not a marker
    let other = fx.user_home.join("repo2");
    fs::create_dir_all(other.join("sub")).unwrap();
    symlink(top.join(".git"), other.join(".git")).unwrap();
    ok(&pin(&fx, &other.join("sub"), &["carton", "0.2"]));
    assert!(other.join("sub/.cig-carton").is_file());
    assert!(!other.join(".cig-carton").exists());
}

// ----- discovery --------------------------------------------------------------------------

fn mkfifo(p: &Path) {
    let st = Command::new("mkfifo").arg(p).status().expect("mkfifo");
    assert!(st.success());
}

#[test]
fn a_fifo_pin_file_is_absent_and_never_hangs() {
    let fx = base();
    for name in [
        ".cig-carton",
        "cig-tasks.json",
        "cigar.toml",
        ".cig-version",
    ] {
        let d = dir(&fx, &format!("fifo-{name}"));
        mkfifo(&d.join(name));
        let o = shim(&fx, "cig", &d);
        ok(&o);
        assert!(out(&o).starts_with("cig 1.20.0 "), "{name}: {}", text(&o));
        let o = which(&fx, &d, &["cig"]);
        ok(&o);
        assert!(text(&o).contains("cig 1.20.0"), "{name}: {}", text(&o));
    }
}

#[test]
fn links_and_pins_above_home_are_ignored_by_both() {
    let mut fx = fixture();
    cartons(&mut fx);
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.2.0"]));
    // a symlinked pin file
    let real = fx.user_home.join("real-pin");
    fs::write(&real, "carton 0.2\n").unwrap();
    let d = dir(&fx, "linked");
    symlink(&real, d.join(".cig-carton")).unwrap();
    // a pin above HOME
    fs::write(fx.tmp.path().join(".cig-carton"), "carton 0.2\n").unwrap();
    let above = dir(&fx, "below");
    for cwd in [&d, &above] {
        let o = shim(&fx, "cig", cwd);
        assert!(out(&o).starts_with("cig 1.20.0 "), "{}", text(&o));
        let o = which(&fx, cwd, &["cig", "--path"]);
        ok(&o);
        assert!(out(&o).contains("/cig/1.20.0/"), "{}", text(&o));
    }
}

#[test]
fn a_malformed_pin_blocks_which_only_and_pin_repairs_it() {
    let mut fx = fixture();
    cartons(&mut fx);
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    let d = dir(&fx, "broken");
    fs::write(d.join(".cig-carton"), "# nothing here\n").unwrap();
    let run = |args: &[&str]| {
        fx.cigi()
            .arg("--yes")
            .args(args)
            .current_dir(&d)
            .output()
            .unwrap()
    };
    let o = run(&["which", "cig"]);
    assert_ne!(o.status.code(), Some(0));
    assert!(text(&o).contains("E1841"), "{}", text(&o));
    ok(&run(&["list"]));
    ok(&{
        let mut c = fx.cigi();
        c.env("CIGI_ALLOW_SINGLE_SOURCE", "true")
            .args(["--yes", "install", "carton", "--carton", "0.2.0"])
            .current_dir(&d);
        c.output().unwrap()
    });
    ok(&run(&["use", "0.2.0"]));
    ok(&run(&["use", "0.1.0"]));
    // pin repairs the file in the same directory
    ok(&run(&["pin", "carton", "0.2"]));
    assert_eq!(read(&d.join(".cig-carton")), "# nothing here\ncarton 0.2\n");
    let o = which(&fx, &d, &["cig", "--path"]);
    ok(&o);
    assert!(out(&o).contains("/cig/1.21.0/"), "{}", text(&o));
    // an invalid cig-tasks.json is not valid enough to edit, and says so; `list` still works
    let e = dir(&fx, "broken2");
    fs::write(e.join("cig-tasks.json"), "{\"carton\": ").unwrap();
    let o = pin(&fx, &e, &["carton", "0.2", "--file", "cig-tasks.json"]);
    assert!(text(&o).contains("E1841"), "{}", text(&o));
    assert_eq!(read(&e.join("cig-tasks.json")), "{\"carton\": ");
    ok(&fx.cigi().arg("list").current_dir(&e).output().unwrap());
}

// ----- the shim and the resolver agree ------------------------------------------------------

/// xorshift64*: deterministic, no dependency.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn pick<'a, T>(&mut self, v: &'a [T]) -> &'a T {
        &v[self.below(v.len() as u64) as usize]
    }
}

const PINS: [&str; 10] = [
    "0.1.0", "0.2.0", "0.1", "0.2", "0", ">=0.1.0", ">=0.2.0", "bogus", "0.9", "v0.2",
];

fn pin_file(rng: &mut Rng, name: &str) -> String {
    let p = *rng.pick(&PINS);
    match name {
        ".cig-carton" => match rng.below(4) {
            0 => format!("# note\ncarton {p}\n"),
            1 => format!("{p}\n"),
            2 => "# nothing\n".to_string(),
            _ => format!("carton {p}\r\n"),
        },
        "cig-tasks.json" => match rng.below(4) {
            0 => format!("{{\"name\": \"x\", \"carton\": \"{p}\"}}\n"),
            1 => format!("{{\"a\": {{\"carton\": \"{p}\"}}}}\n"),
            2 => "{\"name\": \"x\"}\n".to_string(),
            _ => format!("{{\n  \"carton\": \"{p}\"\n}}\n"),
        },
        "cigar.toml" => match rng.below(4) {
            0 => format!("name = \"x\"\ncarton = \"{p}\"\n"),
            1 => format!("m = [\n  [1],\n]\ncarton = '{p}' # c\n[d]\nx = 1\n"),
            2 => format!("[p]\ncarton = \"{p}\"\n"),
            _ => "name = \"x\"\n".to_string(),
        },
        // the legacy file pins the cig build, not the carton
        _ => format!(
            "{}\n",
            rng.pick(&[">=1.21.0", ">=1.20.0", "1.20.0", "nonsense"])
        ),
    }
}

/// The files of a generated layout, for a failure message.
fn describe(root: &Path) -> String {
    let mut s = String::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let md = fs::symlink_metadata(&p).unwrap();
            let what = if md.file_type().is_symlink() {
                format!("-> {:?}", fs::read_link(&p).unwrap())
            } else if md.is_dir() {
                stack.push(p.clone());
                "dir".to_string()
            } else if md.is_file() {
                format!(
                    "{:?}",
                    fs::read(&p).map(|b| String::from_utf8_lossy(&b).into_owned())
                )
            } else {
                "special".to_string()
            };
            s.push_str(&format!("  {}: {what}\n", p.display()));
        }
    }
    s
}

/// Random layouts: nested directories with pin files of every dialect, exact, minor, major and
/// `>=` pins, bad ones, symlinked ones, fifos, and pins above HOME. What the shim runs must be what
/// `ciginstaller which` names (SPEC 8), and when one refuses the other does too.
#[test]
fn the_shim_and_which_agree_on_200_generated_layouts() {
    let mut fx = fixture();
    cartons(&mut fx);
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.2.0"]));
    // above HOME: reaches only the layouts that start outside it
    fs::write(fx.tmp.path().join(".cig-carton"), "carton 0.2\n").unwrap();
    let real = fx.tmp.path().join("targets");
    fs::create_dir_all(&real).unwrap();
    fs::write(real.join("pin-02"), "carton 0.2\n").unwrap();
    fs::write(real.join("pin-01"), "carton 0.1\n").unwrap();
    let names = [
        ".cig-carton",
        "cig-tasks.json",
        "cigar.toml",
        ".cig-version",
    ];
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut runs = (0, 0); // (agreed on a build, agreed on a refusal)
    for n in 0..200 {
        let outside = n % 5 == 4;
        let base_dir = if outside {
            fx.tmp.path().join(format!("out{n}"))
        } else {
            fx.user_home.join(format!("w{n}"))
        };
        let mut cwd = base_dir.clone();
        let depth = 1 + rng.below(4);
        for level in 0..depth {
            fs::create_dir_all(&cwd).unwrap();
            for name in names {
                match rng.below(8) {
                    0 | 1 => fs::write(cwd.join(name), pin_file(&mut rng, name)).unwrap(),
                    2 => {
                        let t = if rng.below(2) == 0 {
                            "pin-01"
                        } else {
                            "pin-02"
                        };
                        symlink(real.join(t), cwd.join(name)).unwrap();
                    }
                    3 if rng.below(6) == 0 => mkfifo(&cwd.join(name)),
                    4 if rng.below(12) == 0 => fs::write(cwd.join(name), b"\xff\xfe\n").unwrap(),
                    _ => {}
                }
            }
            if level + 1 < depth {
                cwd = cwd.join(format!("d{level}"));
            }
        }
        let s = shim(&fx, "cig", &cwd);
        let w = which(&fx, &cwd, &["cig", "--path"]);
        let label = format!(
            "layout {n} at {} (outside HOME: {outside})\n{}",
            cwd.display(),
            describe(&base_dir)
        );
        if w.status.success() {
            let path = out(&w);
            let version = path
                .split("/cig/")
                .nth(1)
                .and_then(|r| r.split('/').next())
                .unwrap_or("");
            assert!(
                s.status.success(),
                "{label}: which says {path}, the shim failed: {}",
                text(&s)
            );
            let said = out(&s);
            assert!(
                said.starts_with(&format!("cig {version} ")),
                "{label}: which says {path}, the shim ran: {said}\n{}",
                text(&s)
            );
            runs.0 += 1;
        } else {
            assert!(
                !s.status.success(),
                "{label}: which refused ({}), the shim ran: {}",
                text(&w),
                text(&s)
            );
            runs.1 += 1;
        }
    }
    // the generator reaches both kinds of verdict
    assert!(runs.0 >= 40 && runs.1 >= 5, "{runs:?}");
}

/// After `rollback cig` the machine carton's cig is the old build. The shim and the resolver must
/// say so alike wherever the chosen carton is the machine carton: no pin, a pin to it, and
/// CIGI_CARTON set to it; the tools that were not rolled back keep the carton's own build.
#[test]
fn a2p_a_rolled_back_tool_agrees_under_a_pin_and_under_cigi_carton() {
    let fx = base();
    ok(&cigi(&fx, &["update", "--no-trial"]));
    ok(&cigi(&fx, &["rollback", "cig"]));
    let proj = dir(&fx, "rb");
    fs::write(proj.join(".cig-carton"), "carton 0.2\n").unwrap();
    let plain = dir(&fx, "rb-plain");
    let version_of = |o: &Output| {
        let t = out(o);
        t.split("/cig/")
            .nth(1)
            .and_then(|r| r.split('/').next())
            .unwrap_or("")
            .to_string()
    };
    for (label, cwd) in [("no pin", &plain), ("pin to the machine carton", &proj)] {
        let s = shim(&fx, "cig", cwd);
        assert!(s.status.success(), "{label}: {}", text(&s));
        assert!(out(&s).starts_with("cig 1.20.0 "), "{label}: {}", text(&s));
        let w = which(&fx, cwd, &["cig", "--path"]);
        ok(&w);
        assert_eq!(version_of(&w), "1.20.0", "{label}: {}", text(&w));
        let why = which(&fx, cwd, &["cig"]);
        ok(&why);
        assert!(
            text(&why).contains("cig 1.20.0, from carton 0.2.0"),
            "{label}: the machine carton with the rolled-back build: {}",
            text(&why)
        );
        // cigo was not rolled back
        let o = shim(&fx, "cigo", cwd);
        assert!(out(&o).starts_with("cigo 1.17.0 "), "{label}: {}", text(&o));
    }
    // CIGI_CARTON names the machine carton: the shim's fast path and the resolver agree
    let bin = fx.cigscript().join("bin");
    let mut c = Command::new(bin.join("cig"));
    c.env_clear()
        .env("HOME", &fx.user_home)
        .env("CIGSCRIPT_HOME", fx.cigscript())
        .env("CIGI_CARTON", "0.2.0")
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .current_dir(&plain);
    let s = run_timeout(&mut c, Duration::from_secs(30));
    assert!(out(&s).starts_with("cig 1.20.0 "), "{}", text(&s));
    let w = run_timeout(
        fx.cigi()
            .env("CIGI_CARTON", "0.2.0")
            .args(["which", "cig", "--path"])
            .current_dir(&plain),
        Duration::from_secs(20),
    );
    ok(&w);
    assert_eq!(version_of(&w), "1.20.0", "{}", text(&w));
}
