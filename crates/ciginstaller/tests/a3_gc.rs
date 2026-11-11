// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `a3_gc` (SPEC 14.3): cartons older than `keep_days` are removed; the machine carton, the
//! previous one and a version a run record names are kept; the store objects of removed
//! toolchains are freed once, and only when nothing else references them.
//!
//! Age is set by writing old dates into `state.json` (fixture data) and the run records' own
//! `started` fields, so the tests run on the real clock: `CIGI_NOW` is honored only by builds
//! with the installer's own `fault-injection` feature, which a plain `cargo test` does not turn on.
#![cfg(unix)]

mod common;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use cigi_testkit::{Fixture, Snapshot};
use common::*;

const OLD: &str = "2020-01-01T00:00:00Z";

/// Seven cartons. Machine 0.4.0, previous 0.1.0. Old (2020): 0.1.0, 0.2.0, 0.3.0, 0.4.0, 0.6.0,
/// 0.7.0. Recent: 0.5.0, which shares cigo 1.17.0 with 0.2.0.
fn setup() -> Fixture {
    let mut fx = fixture();
    for (c, cig, cigo) in [
        ("0.1.0", "1.20.0", "1.16.0"),
        ("0.2.0", "1.21.0", "1.17.0"),
        ("0.3.0", "1.22.0", "1.19.0"),
        ("0.4.0", "1.23.0", "1.18.0"),
        ("0.5.0", "1.24.0", "1.17.0"),
        ("0.6.0", "1.25.0", "1.20.0"),
        ("0.7.0", "1.26.0", "1.21.0"),
    ] {
        fx.carton(c, &[("cig", cig), ("cigo", cigo)]);
    }
    fx.publish("stable");
    for c in [
        "0.1.0", "0.2.0", "0.3.0", "0.4.0", "0.5.0", "0.6.0", "0.7.0",
    ] {
        ok(&cigi(&fx, &["install", "carton", "--carton", c]));
    }
    ok(&cigi(&fx, &["use", "0.4.0"]));
    age(&fx, &["0.1.0", "0.2.0", "0.3.0", "0.4.0", "0.6.0", "0.7.0"]);
    fx
}

/// Make these cartons old: installed and last used in 2020.
fn age(fx: &Fixture, cartons: &[&str]) {
    let p = fx.cigscript().join("installer/state.json");
    let mut s: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
    for c in cartons {
        let rec = &mut s["cartons"][*c];
        rec["installed_at"] = OLD.into();
        rec["last_used"] = serde_json::Value::Null;
    }
    std::fs::write(&p, serde_json::to_string_pretty(&s).unwrap() + "\n").unwrap();
}

/// One run record as cig writes it (the fields gc reads: `version`, `started`).
fn run_record(fx: &Fixture, id: &str, cig: &str, started: &str, pinned: bool) {
    let d = fx.cigscript().join("runs").join(id);
    std::fs::create_dir_all(&d).unwrap();
    let rec = serde_json::json!({
        "id": id, "version": cig, "script": "x.cig", "mode": "run", "started": started,
        "status": "ok", "burns": 0, "irreversible": 0, "rolled_back": false,
    });
    std::fs::write(d.join("run.json"), rec.to_string()).unwrap();
    if pinned {
        std::fs::write(d.join("pinned"), "").unwrap();
    }
}

fn now() -> String {
    cigi_home::now_rfc3339()
}

fn state(fx: &Fixture) -> serde_json::Value {
    serde_json::from_str(
        &std::fs::read_to_string(fx.cigscript().join("installer/state.json")).unwrap(),
    )
    .unwrap()
}

fn cartons(fx: &Fixture) -> Vec<String> {
    state(fx)["cartons"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect()
}

fn toolchain(fx: &Fixture, tool: &str, v: &str) -> bool {
    fx.cigscript()
        .join("toolchains")
        .join(tool)
        .join(v)
        .is_dir()
}

fn objects(fx: &Fixture) -> Vec<(String, u64, u64)> {
    let mut out = Vec::new();
    for shard in std::fs::read_dir(fx.cigscript().join("humidor/store/objects")).unwrap() {
        for f in std::fs::read_dir(shard.unwrap().path()).unwrap() {
            let f = f.unwrap();
            let m = f.metadata().unwrap();
            out.push((
                f.file_name().to_string_lossy().into_owned(),
                m.len(),
                m.nlink(),
            ));
        }
    }
    out.sort();
    out
}

fn json(o: &std::process::Output) -> serde_json::Value {
    serde_json::from_slice(&o.stdout).unwrap_or_else(|e| panic!("{e}: {}", text(o)))
}

fn whole_home(fx: &Fixture) -> Snapshot {
    // the lock file holds the pid of whoever took it last
    Snapshot::take(&fx.cigscript()).without(&[
        "installer/lock",
        "installer.lock",
        "installer/lock.pid",
    ])
}

#[test]
fn a3_gc() {
    let fx = setup();
    // 0.3.0's cig 1.22.0 was run lately; a run of 1.21.0 was long ago, so it protects nothing;
    // a pinned run keeps 0.7.0's cig 1.26.0 however old it is
    run_record(&fx, "20261101T100000-aaaaaa", "1.22.0", &now(), false);
    run_record(&fx, "20200101T100000-bbbbbb", "1.21.0", OLD, false);
    run_record(&fx, "20200101T100000-cccccc", "1.26.0", OLD, true);
    let all: Vec<String> = (1..=7).map(|n| format!("0.{n}.0")).collect();
    assert_eq!(cartons(&fx), all);

    // --dry-run: says what would go and what it would free, writes nothing
    let before = whole_home(&fx);
    let dry = cigi(&fx, &["--json", "gc", "--dry-run"]);
    ok(&dry);
    whole_home(&fx).assert_same(&before);
    let d = json(&dry);
    let gone: Vec<&str> = d["data"]["cartons"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["carton"].as_str().unwrap())
        .collect();
    assert_eq!(gone, ["0.2.0", "0.6.0"]);
    let kept: Vec<(&str, &str)> = d["data"]["kept"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c["carton"].as_str().unwrap(), c["why"].as_str().unwrap()))
        .collect();
    assert!(
        kept.contains(&("0.4.0", "the machine's carton")),
        "{kept:?}"
    );
    assert!(
        kept.contains(&("0.1.0", "the carton rollback returns to")),
        "{kept:?}"
    );
    assert!(
        kept.contains(&("0.3.0", "a run record names its cig")),
        "{kept:?}"
    );
    assert!(
        kept.contains(&("0.7.0", "a run record names its cig")),
        "{kept:?}"
    );
    let plain = cigi(&fx, &["gc", "--dry-run"]);
    ok(&plain);
    assert!(
        text(&plain).contains("dry run: would remove 2 carton(s), 3 toolchain(s)"),
        "{}",
        text(&plain)
    );
    assert!(
        text(&plain).contains("would remove carton 0.2.0 (unused for"),
        "{}",
        text(&plain)
    );
    let (free_objects, free_bytes) = (
        d["data"]["objects"].as_u64().unwrap(),
        d["data"]["bytes"].as_u64().unwrap(),
    );
    assert!(free_objects > 0 && free_bytes > 0);

    // the real thing
    let objs_before = objects(&fx);
    let o = cigi(&fx, &["--json", "gc"]);
    ok(&o);
    let r = json(&o);
    assert_eq!(
        r["data"]["objects"].as_u64().unwrap(),
        free_objects,
        "dry run and run agree"
    );
    assert_eq!(r["data"]["bytes"].as_u64().unwrap(), free_bytes);
    assert_eq!(cartons(&fx), ["0.1.0", "0.3.0", "0.4.0", "0.5.0", "0.7.0"]);
    // 0.2.0's cig and 0.6.0's cig and cigo are gone; cigo 1.17.0 stays: 0.5.0 still has it
    for (t, v, want) in [
        ("cig", "1.21.0", false),
        ("cig", "1.25.0", false),
        ("cigo", "1.20.0", false),
        ("cigo", "1.17.0", true),
        ("cig", "1.20.0", true),
        ("cig", "1.22.0", true),
        ("cig", "1.23.0", true),
        ("cig", "1.24.0", true),
        ("cig", "1.26.0", true),
        ("cigo", "1.18.0", true),
    ] {
        assert_eq!(toolchain(&fx, t, v), want, "{t} {v}");
    }
    // the humidor mirror of a removed cig is gone, a kept one stays
    let idx = std::fs::read_to_string(fx.cigscript().join("humidor/index.json")).unwrap();
    assert!(
        !idx.contains("\"1.21.0\"") && idx.contains("\"1.22.0\""),
        "{idx}"
    );
    assert!(!fx.cigscript().join("humidor/1.21.0").exists());
    // the bytes went: objects freed, none left over, and every remaining object has a link
    let objs_after = objects(&fx);
    assert_eq!(
        (objs_before.len() - objs_after.len()) as u64,
        free_objects,
        "{objs_before:?}"
    );
    let freed_bytes: u64 = objs_before
        .iter()
        .filter(|o| !objs_after.iter().any(|a| a.0 == o.0))
        .map(|o| o.1)
        .sum();
    assert_eq!(freed_bytes, free_bytes);
    assert!(
        objs_after.iter().all(|(_, _, links)| *links >= 2),
        "an object nothing links to is left: {objs_after:?}"
    );
    // nothing in the gc transaction still holds the removed trees
    let held: u64 = Snapshot::take(&fx.cigscript().join("installer/txn"))
        .entries
        .keys()
        .filter(|k| k.contains("1.21.0") || k.contains("1.25.0"))
        .count() as u64;
    assert_eq!(held, 0);
    // history and the clock
    let s = state(&fx);
    assert!(s["gc"]["last_run"].is_string());
    assert_eq!(
        s["history"].as_array().unwrap().last().unwrap()["what"],
        "gc"
    );

    // what is left works: the machine carton, and a project pinned to the shared cigo
    assert_eq!(
        out(&fx.run_shim("cigo", &fx.project, &[])),
        "cigo 1.18.0 uses cig 1.23.0"
    );
    let p = fx.tmp.path().join("pinned");
    std::fs::create_dir_all(&p).unwrap();
    std::fs::write(p.join(".cig-carton"), "carton 0.5\n").unwrap();
    assert_eq!(
        out(&fx.run_shim("cigo", &p, &[])),
        "cigo 1.17.0 uses cig 1.24.0"
    );

    // freed once: a second gc has nothing to free and changes nothing
    let after = whole_home(&fx);
    let o = cigi(&fx, &["--json", "gc"]);
    ok(&o);
    let r = json(&o);
    assert_eq!(r["data"]["objects"], 0);
    assert_eq!(r["data"]["cartons"].as_array().unwrap().len(), 0);
    assert!(text(&o).contains("nothing to remove"), "{}", text(&o));
    whole_home(&fx).assert_same(&after);
}

#[test]
fn a3_gc_keep_days_by_flag_and_by_setting() {
    let fx = setup();
    let before = whole_home(&fx);
    // a long enough keep: nothing is old
    let o = cigi(&fx, &["gc", "--keep-days", "100000"]);
    ok(&o);
    assert!(text(&o).contains("nothing to remove"), "{}", text(&o));
    whole_home(&fx).assert_same(&before);
    // the setting, through its documented environment override
    let o = fx
        .cigi()
        .env("CIGI_ALLOW_SINGLE_SOURCE", "true")
        .env("CIGI_KEEP_DAYS", "100000")
        .args(["--yes", "gc", "--dry-run"])
        .output()
        .unwrap();
    ok(&o);
    assert!(text(&o).contains("nothing to remove"), "{}", text(&o));
    // a bad number is a usage error
    let o = cigi(&fx, &["gc", "--keep-days", "soon"]);
    assert_eq!(o.status.code(), Some(3), "{}", text(&o));
    whole_home(&fx).assert_same(&before);
}

#[test]
fn a3_gc_never_touches_run_records_or_user_data() {
    let fx = setup();
    run_record(&fx, "20200101T100000-bbbbbb", "9.9.9", OLD, false);
    // something that is not a run: a file, a folder with no record, a record that is not JSON
    let runs = fx.cigscript().join("runs");
    std::fs::write(runs.join("stray.txt"), "x").unwrap();
    std::fs::create_dir_all(runs.join("empty")).unwrap();
    std::fs::create_dir_all(runs.join("junk")).unwrap();
    std::fs::write(runs.join("junk/run.json"), "{ not json").unwrap();
    let user = fx.cigscript().join("tin");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(user.join("mine.txt"), "mine").unwrap();
    let snap = |p: &Path| Snapshot::take(p);
    let (r0, t0) = (snap(&runs), snap(&user));
    ok(&cigi(&fx, &["gc"]));
    snap(&runs).assert_same(&r0);
    snap(&user).assert_same(&t0);
}

/// Kill gc at every journal point: the next gc finishes the job. Whatever the kill left, the
/// end state is the same as an uninterrupted run, and no object is left that nothing links to.
#[test]
fn a3_gc_killed_at_every_point_then_replayed() {
    let small = || {
        let mut fx = fixture();
        for (c, cig, cigo) in [
            ("0.1.0", "1.20.0", "1.16.0"),
            ("0.2.0", "1.21.0", "1.17.0"),
            ("0.3.0", "1.22.0", "1.17.0"),
        ] {
            fx.carton(c, &[("cig", cig), ("cigo", cigo)]);
        }
        fx.publish("stable");
        for c in ["0.1.0", "0.2.0", "0.3.0"] {
            ok(&cigi(&fx, &["install", "carton", "--carton", c]));
        }
        ok(&cigi(&fx, &["use", "0.3.0"]));
        age(&fx, &["0.1.0", "0.2.0", "0.3.0"]);
        fx
    };
    // the uninterrupted result
    let clean = small();
    ok(&cigi(&clean, &["gc"]));
    let want_cartons = cartons(&clean);
    let want_objects: Vec<String> = objects(&clean).into_iter().map(|o| o.0).collect();
    assert_eq!(want_cartons, ["0.1.0", "0.3.0"]);

    let mut crashes = 0;
    for phase in [
        "mkdir", "store", "link", "move", "write", "delete", "flip", "flipped", "commit",
    ] {
        for n in 1..40 {
            let fx = small();
            let o = fx
                .cigi()
                .env("CIGI_ALLOW_SINGLE_SOURCE", "true")
                .env("CIGI_FAULT", format!("{phase}:{n}"))
                .args(["--yes", "gc"])
                .output()
                .unwrap();
            if o.status.code().is_some() {
                break;
            }
            crashes += 1;
            let tag = format!("{phase}:{n}");
            // no shim can find an index that names a toolchain already deleted
            let idx = std::fs::read_to_string(fx.cigscript().join("installer/shim-index")).unwrap();
            let root = fx.cigscript().display().to_string();
            for word in idx.split(|c: char| c.is_whitespace()) {
                if word.starts_with(&root) {
                    assert!(Path::new(word).exists(), "{tag}: the index names {word}");
                }
            }
            // the machine still runs whatever the kill left
            assert_eq!(
                out(&fx.run_shim("cigo", &fx.project, &[])),
                "cigo 1.17.0 uses cig 1.22.0",
                "{tag}"
            );
            ok(&cigi(&fx, &["gc"]));
            assert_eq!(cartons(&fx), want_cartons, "{tag}");
            let objs = objects(&fx);
            assert_eq!(
                objs.iter().map(|o| o.0.clone()).collect::<Vec<_>>(),
                want_objects,
                "{tag}"
            );
            assert!(objs.iter().all(|o| o.2 >= 2), "{tag}: {objs:?}");
            assert!(!toolchain(&fx, "cig", "1.21.0"), "{tag}");
            assert!(toolchain(&fx, "cigo", "1.17.0"), "{tag}");
        }
    }
    assert!(crashes >= 6, "only {crashes} fault points reached");
}
