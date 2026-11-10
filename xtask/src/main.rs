// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `cargo xtask`: the maintainer's tools. See `manifest/README.md`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::exit;

use cig_compat::Version;
use xtask::assemble::{assemble, sidecar_line, Request};

const USAGE: &str = "\
usage:
  cargo xtask manifest assemble --carton <v> [--cig <v>] [--cigo <v>] [--cigcompile <v>]
        [--installer <v>] [--tool <name>=<v>]... [--pack <name>=<tool>,<tool>]...
        [--notes <text>] [--channel stable] [--previous <manifest.toml>]
        [--expires-days 30] [--out <file>]
  cargo xtask pins <manifest.toml>
  cargo xtask errors-doc";

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
        _ => usage("what to do?"),
    }
}

fn assemble_cmd(rest: &[&str]) {
    let mut carton = None;
    let mut members = BTreeMap::new();
    let mut packs = BTreeMap::new();
    let (mut notes, mut channel) = (String::new(), "stable".to_string());
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
            "--channel" => channel = value(),
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
    let carton = carton.unwrap_or_else(|| usage("--carton is required"));
    let inputs = root().join("manifest");
    let req = Request {
        channel: channel.clone(),
        carton,
        members,
        packs,
        notes,
        expires_days,
        previous: previous.as_deref().map(read),
        tools_toml: read(&inputs.join("tools.toml")),
        products_toml: read(&inputs.join("products.toml")),
        now: time::OffsetDateTime::now_utc(),
    };
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
