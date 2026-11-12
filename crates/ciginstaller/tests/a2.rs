// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! Phase 2 acceptance (SPEC 14.3): install, shims, pins, resolution.

mod common;

use std::path::Path;

use cigi_home::platform::ino;
use cigi_testkit::{Fixture, Snapshot};
use common::{cigi, fixture, ok, out, text};

fn two_cartons(fx: &mut Fixture) {
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.16.0")]);
    fx.carton("0.2.0", &[("cig", "1.21.0"), ("cigo", "1.17.0")]);
    fx.publish("stable");
}

fn pinned_project(fx: &Fixture, name: &str, pin: &str) -> std::path::PathBuf {
    let p = fx.tmp.path().join(name);
    std::fs::create_dir_all(&p).unwrap();
    std::fs::write(p.join(".cig-carton"), format!("carton {pin}\n")).unwrap();
    p
}

fn shim_out(fx: &Fixture, tool: &str, cwd: &Path) -> String {
    let o = fx.run_shim(tool, cwd, &[]);
    ok(&o);
    out(&o)
}

/// #1's done-when: two cigo versions side by side, one shim choosing by project.
#[test]
fn a2_two_cigo_side_by_side() {
    let mut fx = fixture();
    two_cartons(&mut fx);
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.2.0"]));
    let a = pinned_project(&fx, "A", "0.1");
    let b = pinned_project(&fx, "B", "0.2");
    assert_eq!(shim_out(&fx, "cigo", &a), "cigo 1.16.0 uses cig 1.20.0");
    // cigo run in B calls cig, which is B's carton's cig (CIGI_CARTON propagation)
    assert_eq!(shim_out(&fx, "cigo", &b), "cigo 1.17.0 uses cig 1.21.0");
    // no pin: the machine carton (the first one installed)
    assert_eq!(
        shim_out(&fx, "cigo", &fx.project),
        "cigo 1.16.0 uses cig 1.20.0"
    );

    let h = fx.cigscript().join("toolchains").join("cigo");
    let (x, y) = (
        h.join("1.16.0").join("LICENSE"),
        h.join("1.17.0").join("LICENSE"),
    );
    assert!(
        ino(&x).is_some() && ino(&x) == ino(&y),
        "unchanged files share an inode"
    );

    let o = fx
        .cigi()
        .args(["which", "cigo"])
        .current_dir(&a)
        .output()
        .unwrap();
    ok(&o);
    assert!(
        text(&o).contains("cigo 1.16.0, from carton 0.1.0 (pinned in .cig-carton)"),
        "{}",
        text(&o)
    );
    let o = fx
        .cigi()
        .args(["which", "cigo", "--path"])
        .current_dir(&b)
        .output()
        .unwrap();
    ok(&o);
    assert!(
        out(&o).ends_with("toolchains/cigo/1.17.0/bin/cigo"),
        "{}",
        text(&o)
    );

    // `use` moves the machine; the shim follows
    ok(&cigi(&fx, &["use", "0.2.0"]));
    assert_eq!(
        shim_out(&fx, "cigo", &fx.project),
        "cigo 1.17.0 uses cig 1.21.0"
    );
    let o = cigi(&fx, &["use", "9.9.9"]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(text(&o).contains("E1816"));
}

fn tool_dirs(fx: &Fixture) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(fx.cigscript().join("toolchains"))
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    v.retain(|n| n != "ciginstaller");
    v.sort();
    v
}

fn no_partials(fx: &Fixture) {
    let snap = Snapshot::take(&fx.cigscript().join("toolchains"));
    assert!(
        snap.entries.keys().all(|k| !k.contains(".partial-")),
        "{:?}",
        snap.entries.keys().collect::<Vec<_>>()
    );
}

fn committed_txns(fx: &Fixture) -> usize {
    std::fs::read_dir(fx.cigscript().join("installer").join("txn"))
        .map(|rd| {
            rd.flatten()
                .filter(|e| {
                    std::fs::read_to_string(e.path().join("journal.jsonl"))
                        .is_ok_and(|j| j.contains("\"commit\""))
                })
                .count()
        })
        .unwrap_or(0)
}

#[test]
fn a2_stick_pack_carton() {
    let mut fx = fixture();
    fx.carton(
        "0.1.0",
        &[
            ("cig", "1.20.0"),
            ("cigo", "1.17.0"),
            ("cigcompile", "0.2.0"),
        ],
    );
    fx.publish("stable");
    ok(&cigi(&fx, &["install", "stick", "cig"]));
    assert_eq!(tool_dirs(&fx), ["cig"]);
    ok(&cigi(&fx, &["install", "pack", "dev"]));
    assert_eq!(tool_dirs(&fx), ["cig", "cigo"]);
    ok(&cigi(&fx, &["install", "carton"]));
    assert_eq!(tool_dirs(&fx), ["cig", "cigcompile", "cigo"]);
    no_partials(&fx);
    assert_eq!(committed_txns(&fx), 3, "one transaction each");
    // a repeat is a no-op, not another transaction
    let o = cigi(&fx, &["install", "carton"]);
    ok(&o);
    assert!(text(&o).contains("already installed"), "{}", text(&o));
    assert_eq!(committed_txns(&fx), 3);
    // the shims exist and work
    assert_eq!(
        shim_out(&fx, "cigo", &fx.project),
        "cigo 1.17.0 uses cig 1.20.0"
    );
    let o = cigi(&fx, &["install", "stick", "nope"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(text(&o).contains("E1814"));
}

#[test]
fn a2_legacy_pin() {
    let mut fx = fixture();
    fx.carton("0.1.0", &[("cig", "1.5.0")]);
    fx.carton("0.2.0", &[("cig", "1.20.0")]);
    fx.publish("stable");
    let proj = fx.tmp.path().join("legacy");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(proj.join(".cig-version"), ">=1.6.2\n").unwrap();

    // only 1.5.0 installed: refused with E1801-style text and the stick fix
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    let o = fx.run_shim("cig", &proj, &[]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    let t = text(&o);
    assert!(t.contains("E1801"), "{t}");
    assert!(
        t.contains("ciginstaller install stick cig --carton 0.2.0"),
        "{t}"
    );

    // both installed: the slow path runs 1.20.0, and `which` says why
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.2.0"]));
    let t = shim_out(&fx, "cig", &proj);
    assert!(t.starts_with("cig 1.20.0 args="), "{t}");
    let o = fx
        .cigi()
        .args(["which", "cig"])
        .current_dir(&proj)
        .output()
        .unwrap();
    ok(&o);
    assert!(
        text(&o).contains("cig 1.20.0, from carton 0.2.0 (satisfies .cig-version >=1.6.2)"),
        "{}",
        text(&o)
    );
}

/// What a crash must leave alone or finish: the paths that make up what is installed.
fn installed_view(fx: &Fixture) -> Snapshot {
    let root = fx.cigscript();
    let mut s = Snapshot::default();
    for p in ["bin", "toolchains", "humidor/store/objects", "humidor"] {
        for (k, v) in Snapshot::take(&root.join(p)).entries {
            s.entries.insert(format!("{p}/{k}"), v);
        }
    }
    for f in ["installer/state.json", "installer/shim-index"] {
        for (k, v) in Snapshot::take(&root).entries {
            if k == f {
                s.entries.insert(k, v);
            }
        }
    }
    s
}

#[test]
fn a2_interrupted_install() {
    let setup = |fault: Option<&str>| -> (Fixture, Option<bool>) {
        let mut fx = fixture();
        fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.16.0")]);
        fx.carton("0.2.0", &[("cig", "1.21.0"), ("cigo", "1.17.0")]);
        fx.publish("stable");
        ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
        let Some(f) = fault else { return (fx, None) };
        let o = fx
            .cigi()
            .env("CIGI_ALLOW_SINGLE_SOURCE", "true")
            .env("CIGI_FAULT", f)
            .args(["--yes", "install", "carton", "--carton", "0.2.0"])
            .output()
            .unwrap();
        let crashed = o.status.code().is_none();
        (fx, Some(crashed))
    };
    let before = installed_view(&setup(None).0).entries.len();
    let mut crashes = 0;
    let mut pre_seen = 0;
    let mut post_seen = 0;
    for phase in [
        "mkdir", "store", "link", "move", "write", "block", "flip", "flipped", "commit",
    ] {
        for n in 1..60 {
            let (fx, crashed) = setup(Some(&format!("{phase}:{n}")));
            if crashed != Some(true) {
                break; // this phase has fewer than n lines
            }
            crashes += 1;
            // the next mutating command recovers and says so exactly once, unless the crash came
            // after the commit line was durable: then there is nothing left to recover
            let done = committed_txns(&fx) > 1;
            let o = cigi(
                &fx,
                &["install", "carton", "--carton", "0.2.0", "--dry-run"],
            );
            ok(&o);
            let notes = text(&o).matches("E1832").count();
            assert_eq!(notes, usize::from(!done), "{phase}:{n}: {}", text(&o));
            let state =
                std::fs::read_to_string(fx.cigscript().join("installer/state.json")).unwrap();
            if state.contains("\"0.2.0\"") {
                post_seen += 1;
                // after the flip: whole and working
                let proj = pinned_project(&fx, "p2", "0.2");
                assert_eq!(
                    shim_out(&fx, "cigo", &proj),
                    "cigo 1.17.0 uses cig 1.21.0",
                    "{phase}:{n}"
                );
                let idx =
                    std::fs::read_to_string(fx.cigscript().join("installer/shim-index")).unwrap();
                assert!(idx.contains("carton\t0.2.0\tcigo"), "{phase}:{n}");
            } else {
                pre_seen += 1;
                // before the flip: exactly the 0.1.0 install (same layout and sizes as a clean one)
                let now = installed_view(&fx);
                assert!(
                    now.entries.keys().all(|k| !k.contains("0.2.0")
                        && !k.contains("1.21.0")
                        && !k.contains("1.17.0")),
                    "{phase}:{n}: {:?}",
                    now.entries.keys().collect::<Vec<_>>()
                );
                assert_eq!(now.entries.len(), before, "{phase}:{n}");
                no_partials(&fx);
                let proj = pinned_project(&fx, "p1", "0.1");
                assert_eq!(
                    shim_out(&fx, "cigo", &proj),
                    "cigo 1.16.0 uses cig 1.20.0",
                    "{phase}:{n}"
                );
            }
        }
    }
    assert!(crashes >= 10, "only {crashes} fault points reached");
    assert!(
        pre_seen > 0 && post_seen > 0,
        "pre {pre_seen}, post {post_seen}"
    );
}

/// A FIRST install killed at every fault point: the next command recovers (one note), and the
/// home is either empty (no bin, toolchains, objects, humidor, shim-index, PATH block) or whole.
#[test]
fn a2_interrupted_first_install() {
    let mut crashes = 0;
    let mut empty_seen = 0;
    let mut whole_seen = 0;
    for phase in [
        "mkdir", "store", "link", "move", "write", "block", "flip", "flipped", "commit",
    ] {
        for n in 1..80 {
            let mut fx = fixture();
            fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.16.0")]);
            fx.publish("stable");
            let o = fx
                .cigi()
                .env("CIGI_ALLOW_SINGLE_SOURCE", "true")
                .env("CIGI_FAULT", format!("{phase}:{n}"))
                .args(["--yes", "install", "carton"])
                .output()
                .unwrap();
            if o.status.code().is_some() {
                break; // this phase has fewer than n lines
            }
            crashes += 1;
            let done = committed_txns(&fx) > 0;
            let o = cigi(&fx, &["install", "carton", "--dry-run"]);
            ok(&o);
            assert_eq!(
                text(&o).matches("E1832").count(),
                usize::from(!done),
                "{phase}:{n}: {}",
                text(&o)
            );
            let state = std::fs::read_to_string(fx.cigscript().join("installer/state.json"))
                .unwrap_or_default();
            let profile =
                std::fs::read_to_string(fx.user_home.join(".profile")).unwrap_or_default();
            if state.contains("\"0.1.0\"") {
                whole_seen += 1;
                assert_eq!(
                    shim_out(&fx, "cigo", &fx.project),
                    "cigo 1.16.0 uses cig 1.20.0",
                    "{phase}:{n}"
                );
                let idx =
                    std::fs::read_to_string(fx.cigscript().join("installer/shim-index")).unwrap();
                assert!(idx.contains("carton\t0.1.0\tcigo"), "{phase}:{n}");
            } else {
                empty_seen += 1;
                let now = installed_view(&fx);
                let files: Vec<&String> = now
                    .entries
                    .iter()
                    .filter(|(_, e)| !format!("{e:?}").contains("Dir"))
                    .map(|(k, _)| k)
                    .collect();
                assert!(files.is_empty(), "{phase}:{n}: {files:?}");
                assert!(!profile.contains("cigscript"), "{phase}:{n}: {profile}");
                no_partials(&fx);
            }
        }
    }
    assert!(crashes >= 10, "only {crashes} fault points reached");
    assert!(
        empty_seen > 0 && whole_seen > 0,
        "empty {empty_seen}, whole {whole_seen}"
    );
}

#[test]
fn a2_digest_attacks() {
    let cases: [(&str, &str); 5] = [
        ("corrupt", "E1821"),
        ("bad_sidecar", "E1827"),
        ("missing_sidecar", "E1827"),
        ("truncate", "E1821"),
        ("zip_slip", "E1826"),
    ];
    for (fault, code) in cases {
        let mut fx = fixture();
        {
            let p = fx.product("cigo", "1.16.0");
            match fault {
                "corrupt" => p.corrupt_asset(),
                "bad_sidecar" => p.bad_sidecar(),
                "missing_sidecar" => p.missing_sidecar(),
                "truncate" => p.truncate(),
                _ => p.zip_slip(),
            };
        }
        fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.16.0")]);
        fx.publish("stable");
        let o = cigi(&fx, &["install", "carton"]);
        assert!(!o.status.success(), "{fault}: {}", text(&o));
        assert!(text(&o).contains(code), "{fault}: {}", text(&o));
        let root = fx.cigscript();
        for p in ["toolchains", "bin", "humidor/store/objects", "humidor"] {
            let s = Snapshot::take(&root.join(p)).without(&["store"]);
            assert!(
                s.entries.is_empty(),
                "{fault}: {p} holds {:?}",
                s.entries.keys().collect::<Vec<_>>()
            );
        }
        assert!(!root.join("installer/state.json").exists(), "{fault}");
        assert!(!root.join("installer/shim-index").exists(), "{fault}");
    }
}

#[test]
fn a2_path_block() {
    let mut fx = fixture();
    two_cartons(&mut fx);
    let profile = fx.user_home.join(".profile");
    std::fs::write(&profile, "export FOO=1\n").unwrap();
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.2.0"]));
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.2.0"]));
    let t = std::fs::read_to_string(&profile).unwrap();
    assert!(t.starts_with("export FOO=1\n"), "{t}");
    assert_eq!(t.matches("# >>> cigscript >>>").count(), 1, "{t}");
    assert_eq!(t.matches("# <<< cigscript <<<").count(), 1, "{t}");
    // --no-path leaves a profile alone
    let mut fx2 = fixture();
    two_cartons(&mut fx2);
    ok(&cigi(&fx2, &["install", "carton", "--no-path"]));
    assert!(!fx2.user_home.join(".profile").exists());
}

// Unix only: damages read-only store objects through a Unix mode so that repair must download;
// proves no command that fetches sends anything but the request.
#[cfg(unix)]
#[test]
fn a2_no_telemetry() {
    let mut fx = fixture();
    two_cartons(&mut fx);
    let server = fx.serve_tls();
    let run = |args: &[&str]| {
        let o = fx
            .cigi_with_server(&server)
            .env("CIGI_ALLOW_SINGLE_SOURCE", "true")
            .arg("--yes")
            .args(args)
            .output()
            .unwrap();
        ok(&o);
    };
    run(&["install", "carton", "--carton", "0.1.0"]);
    run(&["install", "pack", "dev", "--carton", "0.2.0"]);
    run(&["carton", "list"]);
    run(&["carton", "show", "0.2.0"]);
    run(&["list"]);
    run(&["which", "cigo"]);
    assert!(!server.requests().is_empty());

    // #19: every other command that can reach a server. `update --check` with a fresh cache
    // asks nothing; `--refresh` asks for the manifest.
    run(&["update", "--check"]);
    server.clear();
    run(&["update", "--check", "--refresh"]);
    assert!(
        server.paths().iter().any(|p| p.contains("manifest-stable")),
        "{:?}",
        server.paths()
    );
    run(&["update", "--no-trial"]);
    run(&["doctor", "--network"]);
    assert!(
        server.paths().iter().any(|p| p.contains("manifest-stable")),
        "doctor --network fetches the manifest: {:?}",
        server.paths()
    );
    // repair that has to download: the store object is damaged as well as the file
    let victim = fx
        .cigscript()
        .join("toolchains/cigo/1.17.0/orchestrate.cig");
    let victim_obj = fx.cigscript().join("toolchains/cigo/1.17.0/bin/cigo");
    for f in [&victim, &victim_obj] {
        std::fs::set_permissions(f, std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap();
        std::fs::write(f, b"damaged in place\n").unwrap();
    }
    server.clear();
    run(&["repair"]);
    assert!(
        server
            .paths()
            .iter()
            .any(|p| p.contains("cig-orchestrator-v1.17.0")),
        "repair re-fetched nothing: {:?}",
        server.paths()
    );
    if let Err(v) = server.assert_no_telemetry() {
        panic!("{}", v.join("\n"));
    }
    // uninstall makes no request at all
    server.clear();
    run(&["uninstall", "cigo"]);
    run(&["uninstall", "--all", "--yes-irreversible"]);
    assert!(
        server.requests().is_empty(),
        "uninstall asked a server: {:?}",
        server.paths()
    );
    if let Err(v) = server.assert_no_telemetry() {
        panic!("{}", v.join("\n"));
    }

    // `self update --check`: the channel offers a newer installer, the one thing asked is the
    // manifest, and nothing identifies the machine
    let mut fx = fixture();
    fx.product("ciginstaller", "0.1.1");
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.16.0")]);
    fx.carton(
        "0.1.1",
        &[
            ("cig", "1.20.0"),
            ("cigo", "1.16.0"),
            ("ciginstaller", "0.1.1"),
        ],
    );
    fx.publish("stable");
    let server = fx.serve_tls();
    let run = |args: &[&str]| {
        let o = fx
            .cigi_with_server(&server)
            .env("CIGI_ALLOW_SINGLE_SOURCE", "true")
            .arg("--yes")
            .args(args)
            .output()
            .unwrap();
        ok(&o);
    };
    run(&["install", "carton", "--carton", "0.1.0"]);
    server.clear();
    run(&["self", "update", "--check"]);
    assert!(
        server.paths().iter().all(|p| p.contains("manifest-stable")),
        "{:?}",
        server.paths()
    );
    if let Err(v) = server.assert_no_telemetry() {
        panic!("{}", v.join("\n"));
    }
}

/// `install pack dev` on a fresh home: the pack's two tools, one transaction, working shims. How
/// long it takes is a budget, judged on a release build in `cigi-budgets/tests/release_install.rs`
/// (`docs/BENCH.md`), never here: a slow machine is not a failing install.
#[test]
fn a2_install_pack_dev_on_a_fresh_home() {
    let mut fx = fixture();
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.17.0")]);
    fx.publish("stable");
    ok(&cigi(&fx, &["install", "pack", "dev"]));
    assert_eq!(tool_dirs(&fx), ["cig", "cigo"]);
    no_partials(&fx);
    assert_eq!(committed_txns(&fx), 1);
    assert_eq!(
        shim_out(&fx, "cigo", &fx.project),
        "cigo 1.17.0 uses cig 1.20.0"
    );
}

#[test]
fn a2_pin_list_carton_show() {
    let mut fx = fixture();
    two_cartons(&mut fx);
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    let proj = fx.tmp.path().join("pj");
    std::fs::create_dir_all(proj.join(".git")).unwrap();
    let sub = proj.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    let o = fx
        .cigi()
        .args(["pin", "carton", "0.1"])
        .current_dir(&sub)
        .output()
        .unwrap();
    ok(&o);
    assert_eq!(
        std::fs::read_to_string(proj.join(".cig-carton")).unwrap(),
        "carton 0.1\n"
    );
    let o = fx
        .cigi()
        .args(["pin", "cigcompile", "1.0"])
        .current_dir(&sub)
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2));
    assert!(text(&o).contains("E1843"));
    let o = fx
        .cigi()
        .args(["pin", "cig", ">=1.6.2"])
        .current_dir(&sub)
        .output()
        .unwrap();
    ok(&o);
    assert_eq!(
        std::fs::read_to_string(proj.join(".cig-version")).unwrap(),
        ">=1.6.2\n"
    );
    let o = fx
        .cigi()
        .args(["pin", "carton", "nonsense"])
        .current_dir(&sub)
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(3));

    let o = fx.cigi().args(["list"]).output().unwrap();
    ok(&o);
    assert!(text(&o).contains("carton 0.1.0 (machine)"), "{}", text(&o));
    let o = fx
        .cigi()
        .args(["--json", "carton", "list"])
        .output()
        .unwrap();
    ok(&o);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["data"]["cartons"].as_array().map(Vec::len), Some(2));
    let o = fx
        .cigi()
        .args(["carton", "show", "0.2.0"])
        .output()
        .unwrap();
    ok(&o);
    assert!(text(&o).contains("cigo 1.17.0"));
    let o = fx
        .cigi()
        .args(["carton", "show", "7.0.0"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2));
    assert!(text(&o).contains("E1811"));
}

/// The default trust: a mirror that serves the manifest, the checksums and the files itself
/// is one source, and one source is not two that agree. Without the opt-in the install is
/// refused (E1822) and nothing is installed.
#[test]
fn a2_single_source_refused() {
    let mut fx = fixture();
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.16.0")]);
    fx.publish("stable");
    let o = fx
        .cigi()
        .args(["--yes", "install", "carton"])
        .output()
        .expect("run ciginstaller");
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(text(&o).contains("E1822"), "{}", text(&o));
    assert!(tool_dirs(&fx).is_empty(), "nothing is installed");
}
