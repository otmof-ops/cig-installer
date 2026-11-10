// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `cargo xtask`: the maintainer's tools. See `manifest/README.md`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::exit;

use cig_compat::{Manifest, Version};
use xtask::assemble::{assemble, latest_members, next_candidate, sidecar_line, Kind, Request};

const USAGE: &str = "\
usage:
  cargo xtask manifest assemble --carton <v> [--cig <v>] [--cigo <v>] [--cigcompile <v>]
        [--installer <v>] [--tool <name>=<v>]... [--pack <name>=<tool>,<tool>]...
        [--notes <text>] [--channel stable] [--previous <manifest.toml>]
        [--expires-days 30] [--out <file>]
        [--latest]                  (every tool not named: its newest build)
  cargo xtask manifest assemble --candidate beta|nightly --base <X.Y.Z> [the flags above]
        (the carton is numbered X.Y.Z-beta.N or X.Y.Z-nightly.N, N one past the previous
        manifest's; the channel is the candidate's; a nightly implies --latest)
  cargo xtask pins <manifest.toml>
  cargo xtask errors-doc
  cargo xtask fixture-mirror <dir>
  cargo xtask tokens [--check]";

fn fail(msg: &str) -> ! {
    eprintln!("xtask: {msg}");
    exit(1)
}

fn usage(msg: &str) -> ! {
    eprintln!("xtask: {msg}\n{USAGE}");
    exit(3)
}

/// The workspace root: xtask lives one level below it.
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_else(|e| fail(&format!("{}: {e}", p.display())))
}

fn version(s: &str, what: &str) -> Version {
    Version::parse(s).unwrap_or_else(|e| usage(&format!("{what} `{s}`: {e}")))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["manifest", "assemble", rest @ ..] => assemble_cmd(rest),
        ["pins", manifest] => pins_cmd(Path::new(manifest)),
        ["errors-doc"] => errors_doc_cmd(),
        ["fixture-mirror", dir] => fixture_mirror_cmd(Path::new(dir)),
        ["tokens"] => tokens_cmd(false),
        ["tokens", "--check"] => tokens_cmd(true),
        _ => usage("what to do?"),
    }
}

/// Write the testkit's fixture mirror (the one the cargo tests install from) into `dir`, for CI
/// jobs that install from it on machines that have no Rust.
fn fixture_mirror_cmd(dir: &Path) {
    match xtask::fixture_mirror::write(dir) {
        Ok(n) => println!("fixture mirror: {n} files in {}", dir.display()),
        Err(e) => fail(&e),
    }
}

/// Build `crates/cigi-ui/src/tokens.rs` and `design/generated/` from `design/tokens.json`.
fn tokens_cmd(check: bool) {
    if let Err(e) = xtask::tokens::run(&root(), check) {
        fail(&e);
    }
}

fn assemble_cmd(rest: &[&str]) {
    let mut carton = None;
    let mut members = BTreeMap::new();
    let mut packs = BTreeMap::new();
    let (mut notes, mut channel) = (String::new(), None::<String>);
    let (mut candidate, mut base, mut latest) = (None, None, false);
    let (mut previous, mut out, mut expires_days) = (None, None, 30i64);
    let mut it = rest.iter();
    while let Some(flag) = it.next() {
        let mut value = || {
            it.next()
                .map(|s| s.to_string())
                .unwrap_or_else(|| usage(&format!("{flag} needs a value")))
        };
        match *flag {
            "--carton" => carton = Some(version(&value(), "--carton")),
            "--cig" | "--cigo" | "--cigcompile" | "--installer" => {
                let tool = match *flag {
                    "--installer" => "ciginstaller",
                    f => &f[2..],
                };
                members.insert(tool.to_string(), version(&value(), flag));
            }
            "--tool" => {
                let v = value();
                let (t, ver) = v
                    .split_once('=')
                    .unwrap_or_else(|| usage("--tool takes <name>=<version>"));
                members.insert(t.to_string(), version(ver, "--tool"));
            }
            "--pack" => {
                let v = value();
                let (name, tools) = v
                    .split_once('=')
                    .unwrap_or_else(|| usage("--pack takes <name>=<tool>,<tool>"));
                packs.insert(
                    name.to_string(),
                    tools.split(',').map(str::to_string).collect::<Vec<_>>(),
                );
            }
            "--notes" => notes = value(),
            "--channel" => channel = Some(value()),
            "--candidate" => {
                let v = value();
                candidate = Some(
                    Kind::parse(&v).unwrap_or_else(|| usage("--candidate is beta or nightly")),
                );
            }
            "--base" => base = Some(version(&value(), "--base")),
            "--latest" => latest = true,
            "--previous" => previous = Some(PathBuf::from(value())),
            "--out" => out = Some(PathBuf::from(value())),
            "--expires-days" => {
                expires_days = value()
                    .parse()
                    .unwrap_or_else(|_| usage("--expires-days takes a number"))
            }
            other => usage(&format!("unknown flag {other}")),
        }
    }
    let previous_text = previous.as_deref().map(read);
    let channel = match (candidate, channel) {
        (Some(k), Some(c)) if c != k.label() => usage(&format!(
            "--channel {c} does not match --candidate {}",
            k.label()
        )),
        (Some(k), _) => k.label().to_string(),
        (None, c) => c.unwrap_or_else(|| "stable".to_string()),
    };
    let carton = match (candidate, base, carton) {
        (Some(_), _, Some(_)) => usage("--candidate numbers the carton: drop --carton"),
        (Some(k), Some(b), None) => {
            let prev = previous_text.as_deref().map(|t| {
                Manifest::from_str(t)
                    .unwrap_or_else(|e| fail(&format!("the previous manifest: {e}")))
            });
            let n = next_candidate(&b, k, prev.as_ref()).unwrap_or_else(|e| fail(&e));
            println!("- carton {n}: the next {} candidate of {b}", k.label());
            n
        }
        (Some(_), None, None) => usage("--candidate needs --base <X.Y.Z>"),
        (None, Some(_), _) => usage("--base goes with --candidate"),
        (None, None, c) => c.unwrap_or_else(|| usage("--carton is required")),
    };
    let inputs = root().join("manifest");
    let mut req = Request {
        channel: channel.clone(),
        carton,
        members,
        packs,
        notes,
        expires_days,
        previous: previous_text,
        tools_toml: read(&inputs.join("tools.toml")),
        products_toml: read(&inputs.join("products.toml")),
        now: time::OffsetDateTime::now_utc(),
    };
    if latest || candidate == Some(Kind::Nightly) {
        let (members, lines) =
            latest_members(&req, &xtask::gh::Gh, channel != "stable").unwrap_or_else(|e| fail(&e));
        for line in lines {
            println!("- {line}");
        }
        req.members = members;
    }
    let done = assemble(&req, &xtask::gh::Gh).unwrap_or_else(|e| fail(&e));
    let out = out.unwrap_or_else(|| PathBuf::from(format!("manifest-{channel}.toml")));
    let name = out
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| fail("--out names no file"));
    let side = out.with_file_name(format!("{name}.sha256"));
    std::fs::write(&out, &done.text).unwrap_or_else(|e| fail(&format!("{}: {e}", out.display())));
    std::fs::write(&side, sidecar_line(&done.sha256, &name))
        .unwrap_or_else(|e| fail(&format!("{}: {e}", side.display())));
    for line in &done.report {
        println!("- {line}");
    }
    println!("wrote {} and {}", out.display(), side.display());
    println!("read it before uploading it to the `manifest` release");
}

fn pins_cmd(manifest: &Path) {
    let text = xtask::pins::pins_for(&read(manifest)).unwrap_or_else(|e| fail(&e));
    let dest = root().join("crates/cigi-source/src/pins_generated.rs");
    std::fs::write(&dest, text).unwrap_or_else(|e| fail(&format!("{}: {e}", dest.display())));
    println!(
        "wrote {}; review the diff, then build",
        dest.strip_prefix(root()).unwrap_or(&dest).display()
    );
}

fn errors_doc_cmd() {
    let dest = root().join("docs/ERRORS.md");
    let text = xtask::errors_doc::render(cigi_ui::registry::registry());
    std::fs::write(&dest, text).unwrap_or_else(|e| fail(&format!("{}: {e}", dest.display())));
    println!("wrote docs/ERRORS.md from errors/registry.toml");
}
