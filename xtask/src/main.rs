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
  cargo xtask manifest assemble --carton <v> [--cig <v>] [--cigo <v>] [--cigcompile <v>] [--cigar <v>]
        [--installer <v>] [--tool <name>=<v>]... [--pack <name>=<tool>,<tool>]...
        [--notes <text>] [--channel stable] [--previous <manifest.toml>]
        [--expires-days 30] [--out <file>]
        [--latest]                  (every tool not named: its newest build)
  cargo xtask manifest assemble --candidate beta|nightly --base <X.Y.Z> [the flags above]
        (the carton is numbered X.Y.Z-beta.N or X.Y.Z-nightly.N, N one past the previous
        manifest's; the channel is the candidate's; a nightly implies --latest)
        [--tuf-entries <file>]      (also write the signed-target entries for `tuf sign releases`)
  cargo xtask tuf keygen <file> [--base64]
  cargo xtask tuf init --dir <d> --root-key <f>... --root-threshold <n> --targets-key <f>
        --releases-key <f> --snapshot-key <f> --timestamp-key <f>
  cargo xtask tuf sign targets|snapshot|timestamp --dir <d> --key <f> [--expires-days <n>]
  cargo xtask tuf sign releases --dir <d> --key <f> --entries <file> [--expires-days <n>]
  cargo xtask tuf rotate root|targets|snapshot|timestamp --dir <d> --new-key <f>...
        --old-root-key <f>... [--root-threshold <n>]
  cargo xtask tuf rotate releases --dir <d> --new-key <f> --targets-key <f>
  cargo xtask tuf floor --dir <d>
        (key files are paths outside this repository; a key is never printed)
  cargo xtask pins <manifest.toml>
  cargo xtask release prune [--keep 3] [--product <tool>] [--manifest <file>] [--pin <tool>=<v>:<who>]...
        (prints the plan of #26, a dry run; --yes is blocked until the index is signed)
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
        ["release", "prune", rest @ ..] => prune_cmd(rest),
        ["tuf", rest @ ..] => tuf_cmd(rest),
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
    let mut tuf_entries = None::<PathBuf>;
    let mut it = rest.iter();
    while let Some(flag) = it.next() {
        let mut value = || {
            it.next()
                .map(|s| s.to_string())
                .unwrap_or_else(|| usage(&format!("{flag} needs a value")))
        };
        match *flag {
            "--carton" => carton = Some(version(&value(), "--carton")),
            "--cig" | "--cigo" | "--cigcompile" | "--cigar" | "--installer" => {
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
            "--tuf-entries" => tuf_entries = Some(PathBuf::from(value())),
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
    if let Some(p) = tuf_entries {
        let entries = xtask::tuf::entries_for(&done.text).unwrap_or_else(|e| fail(&e));
        let json = serde_json::Value::Array(entries.iter().map(|e| e.to_json()).collect());
        std::fs::write(&p, serde_json::to_string_pretty(&json).unwrap_or_default())
            .unwrap_or_else(|e| fail(&format!("{}: {e}", p.display())));
        println!(
            "wrote {}: {} signed-target entries; `tuf sign releases --entries` signs them",
            p.display(),
            entries.len()
        );
    }
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

/// The plan of #26 for every product (or `--product`): a dry run, nothing else.
fn prune_cmd(rest: &[&str]) {
    use xtask::prune::{self, Options};
    let (mut keep, mut product, mut manifest) = (3usize, None, None::<PathBuf>);
    let mut pins: BTreeMap<(String, Version), Vec<String>> = BTreeMap::new();
    let mut it = rest.iter();
    while let Some(flag) = it.next() {
        let mut value = || {
            it.next()
                .map(|s| s.to_string())
                .unwrap_or_else(|| usage(&format!("{flag} needs a value")))
        };
        match *flag {
            "--keep" => {
                keep = value()
                    .parse()
                    .unwrap_or_else(|_| usage("--keep takes a number"))
            }
            "--product" => product = Some(value()),
            "--manifest" => manifest = Some(PathBuf::from(value())),
            "--pin" => {
                let (key, who) = prune::parse_pin(&value()).unwrap_or_else(|e| usage(&e));
                pins.entry(key).or_default().push(who);
            }
            "--dry-run" => {}
            "--yes" => fail(prune::YES_BLOCKED),
            "--help" | "-h" => {
                println!("{}", prune::HELP);
                return;
            }
            other => usage(&format!("unknown flag {other}")),
        }
    }
    if let Some(path) = manifest {
        let m = Manifest::from_str(&read(&path))
            .unwrap_or_else(|e| fail(&format!("{}: {e}", path.display())));
        for (key, who) in prune::pins_from_manifest(&m) {
            pins.entry(key).or_default().extend(who);
        }
    }
    let tools =
        prune::tools_of(&read(&root().join("manifest/tools.toml"))).unwrap_or_else(|e| fail(&e));
    let opts = Options {
        keep,
        product,
        pins,
    };
    let plans = prune::plan(&xtask::gh::Gh, &tools, &opts).unwrap_or_else(|e| fail(&e));
    print!("{}", prune::render(&plans, keep));
}

/// A key file named on the command line may not be inside this repository.
fn key_path(s: &str) -> PathBuf {
    let p = PathBuf::from(s);
    let abs = p.canonicalize().unwrap_or_else(|_| p.clone());
    let repo = root().canonicalize().unwrap_or_else(|_| root());
    if abs.starts_with(&repo) {
        fail(&format!(
            "{s} is inside the repository; keys live outside it, and a path that is not is the only kind this tool reads"
        ));
    }
    p
}

fn tuf_cmd(rest: &[&str]) {
    use xtask::tuf::{self, KeyFile, Rotate};
    let (cmd, rest) = match rest.split_first() {
        Some((c, r)) => (*c, r),
        None => usage("tuf what?"),
    };
    let (sub, rest) = match (cmd, rest.split_first()) {
        ("sign" | "rotate", Some((s, r))) => (Some(*s), r),
        ("keygen", _) => (None, rest),
        (_, _) => (None, rest),
    };
    let (mut dir, mut keys, mut old_root, mut new_keys) = (None, vec![], vec![], vec![]);
    let (mut root_keys, mut positional) = (vec![], vec![]);
    let (mut threshold, mut entries, mut expires) = (None::<u64>, None::<PathBuf>, None::<i64>);
    let (mut targets_key, mut releases_key, mut snapshot_key, mut timestamp_key) =
        (None, None, None, None);
    let mut b64 = false;
    let mut it = rest.iter();
    while let Some(flag) = it.next() {
        let mut value = || {
            it.next()
                .map(|s| s.to_string())
                .unwrap_or_else(|| usage(&format!("{flag} needs a value")))
        };
        match *flag {
            "--dir" => dir = Some(PathBuf::from(value())),
            "--key" => keys.push(key_path(&value())),
            "--root-key" => root_keys.push(key_path(&value())),
            "--old-root-key" => old_root.push(key_path(&value())),
            "--new-key" => new_keys.push(key_path(&value())),
            "--targets-key" => targets_key = Some(key_path(&value())),
            "--releases-key" => releases_key = Some(key_path(&value())),
            "--snapshot-key" => snapshot_key = Some(key_path(&value())),
            "--timestamp-key" => timestamp_key = Some(key_path(&value())),
            "--root-threshold" => {
                threshold = Some(value().parse().unwrap_or_else(|_| usage("a number")))
            }
            "--entries" => entries = Some(PathBuf::from(value())),
            "--expires-days" => {
                expires = Some(value().parse().unwrap_or_else(|_| usage("a number")))
            }
            "--base64" => b64 = true,
            f if f.starts_with("--") => usage(&format!("unknown flag {f}")),
            p => positional.push(p.to_string()),
        }
    }
    let dir = || dir.clone().unwrap_or_else(|| usage("--dir is required"));
    let key = |p: &Option<PathBuf>, n: &str| {
        KeyFile::read(
            p.as_ref()
                .unwrap_or_else(|| usage(&format!("{n} is required"))),
        )
        .unwrap_or_else(|e| fail(&e))
    };
    let read_keys = |v: &[PathBuf]| -> Vec<KeyFile> {
        v.iter()
            .map(|p| KeyFile::read(p).unwrap_or_else(|e| fail(&e)))
            .collect()
    };
    let now = jiff::Timestamp::now();
    let done = |what: &str, r: Result<u64, String>| match r {
        Ok(v) => println!("wrote {what} version {v} in {}", dir().display()),
        Err(e) => fail(&e),
    };
    match (cmd, sub) {
        ("keygen", _) => {
            let p = positional.first().unwrap_or_else(|| usage("keygen <file>"));
            let id = tuf::keygen(&key_path(p), b64).unwrap_or_else(|e| fail(&e));
            println!("wrote a new key to {p}; key id {id}");
        }
        ("init", _) => {
            let d = dir();
            let r = tuf::init(&tuf::Init {
                dir: &d,
                root_keys: read_keys(&root_keys),
                root_threshold: threshold.unwrap_or_else(|| usage("--root-threshold")),
                targets_key: key(&targets_key, "--targets-key"),
                releases_key: key(&releases_key, "--releases-key"),
                snapshot_key: key(&snapshot_key, "--snapshot-key"),
                timestamp_key: key(&timestamp_key, "--timestamp-key"),
                now,
            });
            match r {
                Ok(()) => println!("wrote 1.root.json and 1.targets.json in {}", d.display()),
                Err(e) => fail(&e),
            }
        }
        ("sign", Some(role)) => {
            let one = || {
                if keys.len() != 1 {
                    usage("give --key once");
                }
                KeyFile::read(&keys[0]).unwrap_or_else(|e| fail(&e))
            };
            match role {
                "targets" => done(
                    "targets",
                    tuf::sign_targets(&dir(), &one(), now, expires.unwrap_or(tuf::TARGETS_DAYS)),
                ),
                "snapshot" => done(
                    "snapshot",
                    tuf::sign_snapshot(&dir(), &one(), now, expires.unwrap_or(tuf::SNAPSHOT_DAYS)),
                ),
                "timestamp" => done(
                    "timestamp",
                    tuf::sign_timestamp(
                        &dir(),
                        &one(),
                        now,
                        expires.unwrap_or(tuf::TIMESTAMP_DAYS),
                    ),
                ),
                "releases" => {
                    let p = entries.unwrap_or_else(|| usage("--entries is required"));
                    let v: serde_json::Value =
                        serde_json::from_str(&read(&p)).unwrap_or_else(|e| fail(&e.to_string()));
                    let list = v
                        .as_array()
                        .unwrap_or_else(|| fail("the entries file is not a list"));
                    let es: Vec<_> = list
                        .iter()
                        .map(|x| tuf::Entry::from_json(x).unwrap_or_else(|e| fail(&e)))
                        .collect();
                    done(
                        "releases",
                        tuf::sign_releases(
                            &dir(),
                            &one(),
                            &es,
                            now,
                            expires.unwrap_or(tuf::RELEASES_DAYS),
                        ),
                    )
                }
                _ => usage("sign targets|releases|snapshot|timestamp"),
            }
        }
        ("rotate", Some("releases")) => {
            if new_keys.len() != 1 {
                usage("give --new-key once");
            }
            done(
                "targets",
                tuf::rotate_releases(
                    &dir(),
                    &key(&targets_key, "--targets-key"),
                    &KeyFile::read(&new_keys[0]).unwrap_or_else(|e| fail(&e)),
                    now,
                ),
            )
        }
        ("rotate", Some(role)) => {
            let what = match role {
                "root" => Rotate::Root,
                "targets" => Rotate::Targets,
                "snapshot" => Rotate::Snapshot,
                "timestamp" => Rotate::Timestamp,
                _ => usage("rotate root|targets|snapshot|timestamp|releases"),
            };
            let d = dir();
            done(
                "root",
                tuf::rotate(&tuf::RotateSpec {
                    dir: &d,
                    what,
                    new_keys: read_keys(&new_keys),
                    root_threshold: threshold,
                    old_root_keys: read_keys(&old_root),
                    now,
                }),
            )
        }
        ("floor", _) => print!("{}", tuf::floor_lines(&dir()).unwrap_or_else(|e| fail(&e))),
        _ => usage("tuf keygen|init|sign|rotate|floor"),
    }
}
