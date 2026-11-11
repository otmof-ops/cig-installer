// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! Staged rollouts (#17): `rollout = N%` decides, from a hash of the machine id that never leaves
//! the machine, whether this machine takes a new carton on its own. The person's own `update`
//! always wins. `status --json` is the inventory, and it makes no request.
//!
//! The binary is given a synthetic id through `CIGI_TEST_MACHINE_ID` (a test seam that can only
//! move this machine's own bucket). The products are the fixture's fakes, so these tests are Unix
//! only.
#![cfg(unix)]

mod common;

use std::path::Path;
use std::process::{Command, Output, Stdio};

use cigi_home::rollout::{bucket, in_share, ID_SEAM};
use cigi_testkit::Fixture;
use common::*;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Two cartons on stable: 0.1.0 installed, 0.1.1 on offer.
fn two(fx: &mut Fixture) {
    fx.product("cig", "1.20.0").dry_run_output("plan: 1 op");
    fx.product("cig", "1.21.0").dry_run_output("plan: 1 op");
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.17.0")]);
    fx.carton("0.1.1", &[("cig", "1.21.0"), ("cigo", "1.17.0")]);
    fx.publish("stable");
}

fn installed() -> Fixture {
    let mut fx = fixture();
    two(&mut fx);
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    fx
}

/// An id whose bucket satisfies `want`.
fn id_where(want: impl Fn(u8) -> bool) -> String {
    (0..100_000u32)
        .map(|i| format!("synthetic-machine-{i:05}"))
        .find(|id| want(bucket(id)))
        .expect("an id in range")
}

fn json(o: &Output) -> Value {
    serde_json::from_slice(&o.stdout).unwrap_or_else(|e| panic!("{e}: {}", text(o)))
}

/// One run with this synthetic id and rollout; `None` for no id at all (the seam set to empty,
/// which is "this machine has no id", so a real /etc/machine-id never decides a test).
fn run(fx: &Fixture, id: Option<&str>, rollout: &str, args: &[&str]) -> Output {
    fx.cigi()
        .env("CIGI_ALLOW_SINGLE_SOURCE", "true")
        .env(ID_SEAM, id.unwrap_or(""))
        .env("CIGI_ROLLOUT", rollout)
        .arg("--yes")
        .args(args)
        .output()
        .expect("run ciginstaller")
}

/// A run with a terminal on stdout and stderr, as the notice wants.
fn on_tty(fx: &Fixture, id: Option<&str>, rollout: &str, args: &str) -> (i32, String) {
    let c = fx.cigi();
    let exe = c.get_program().to_string_lossy().to_string();
    let line = format!("'{}' {args}", exe.replace('\'', "'\\''"));
    let mut s = Command::new("script");
    s.env_clear();
    for (k, v) in c.get_envs() {
        if let Some(v) = v {
            s.env(k, v);
        }
    }
    s.env("CIGI_ALLOW_SINGLE_SOURCE", "true")
        .env(ID_SEAM, id.unwrap_or(""))
        .env("CIGI_ROLLOUT", rollout)
        .args(cigi_testkit::script_args(&line))
        .current_dir(&fx.project)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // the shared helper kills a child that waits at a prompt after 30 seconds
    type_at_prompts(s, &[])
}

fn which_carton(fx: &Fixture) -> String {
    let o = cigi(fx, &["--json", "status"]);
    ok(&o);
    json(&o)["data"]["carton"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

fn digest_hex(id: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"cigi/rollout/v1\0");
    h.update(id.as_bytes());
    hex::encode(h.finalize())
}

// ---- the hash, through the binary and on its own ---------------------------------------------

#[test]
fn a6_the_same_id_always_lands_in_the_same_bucket() {
    let fx = installed();
    let id = id_where(|b| (20..80).contains(&b));
    let b = bucket(&id);
    // N = bucket + 1 is in; N = bucket is out: the boundary is the bucket, whatever the run
    for _ in 0..3 {
        let inside = json(&run(
            &fx,
            Some(&id),
            &(b + 1).to_string(),
            &["--json", "status"],
        ));
        assert_eq!(inside["data"]["rollout"]["in_share"], true, "{inside}");
        assert_eq!(inside["data"]["rollout"]["percent"], b + 1);
        let outside = json(&run(
            &fx,
            Some(&id),
            &format!("{b}%"),
            &["--json", "status"],
        ));
        assert_eq!(outside["data"]["rollout"]["in_share"], false, "{outside}");
        assert_eq!(outside["data"]["rollout"]["reason"], Value::Null);
    }
    // case and spacing of the id do not move the machine
    let loud = format!("  {}\n", id.to_uppercase());
    let again = json(&run(
        &fx,
        Some(&loud),
        &format!("{b}%"),
        &["--json", "status"],
    ));
    assert_eq!(again["data"]["rollout"]["in_share"], false);
}

#[test]
fn a6_buckets_of_many_ids_spread_evenly() {
    let n = 10_000u32;
    let mut deciles = [0u32; 10];
    for i in 0..n {
        let id = format!(
            "{:08x}-{:04x}-{:04x}",
            i.wrapping_mul(2_654_435_761),
            i % 7919,
            i
        );
        deciles[(bucket(&id) / 10) as usize] += 1;
    }
    for (d, c) in deciles.iter().enumerate() {
        assert!((850..=1150).contains(c), "decile {d} has {c}: {deciles:?}");
    }
    // a 10% share takes about a tenth
    let taken = (0..n)
        .filter(|i| in_share(Some(&format!("machine-{i}")), 10))
        .count();
    assert!((850..=1150).contains(&taken), "{taken}");
}

// ---- the decision ----------------------------------------------------------------------------

#[test]
fn a6_full_rollout_notifies_everyone_and_a_share_below_the_bucket_waits() {
    let fx = installed();
    let id = id_where(|b| b >= 50);
    // 100% (the default): the plain notice, even for a high bucket
    let (code, t) = on_tty(&fx, Some(&id), "100", "list");
    assert_eq!(code, 0, "{t}");
    assert!(t.contains("carton 0.1.1 is out"), "{t}");
    assert!(!t.contains("rolling out"), "{t}");
    // 10%: this machine's bucket is 50 or more, so it is told one line and the carton waits
    std::fs::remove_file(fx.cigscript().join("installer/notice.json")).unwrap();
    let (code, t) = on_tty(&fx, Some(&id), "10%", "list");
    assert_eq!(code, 0, "{t}");
    assert!(
        t.contains("carton 0.1.1 is rolling out, 10% of machines so far"),
        "{t}"
    );
    assert!(t.contains("to take it now: ciginstaller update"), "{t}");
    assert!(!t.contains("is out"), "{t}");
    assert_eq!(which_carton(&fx), "0.1.0", "nothing was taken");
    // a machine inside a 10% share gets the plain line
    std::fs::remove_file(fx.cigscript().join("installer/notice.json")).unwrap();
    let low = id_where(|b| b < 10);
    let (_, t) = on_tty(&fx, Some(&low), "10%", "list");
    assert!(t.contains("carton 0.1.1 is out"), "{t}");
}

#[test]
fn a6_an_explicit_update_wins_and_says_so() {
    let fx = installed();
    let id = id_where(|b| b >= 50);
    // --check and --dry-run say it too, in the same words
    for flag in ["--check", "--dry-run"] {
        let o = run(&fx, Some(&id), "10%", &["--json", "update", flag]);
        ok(&o);
        let d = json(&o);
        assert_eq!(d["data"]["rollout"]["in_share"], false, "{d}");
        assert_eq!(d["data"]["rollout"]["explicit_override"], true, "{d}");
    }
    let o = run(&fx, Some(&id), "10%", &["update"]);
    ok(&o);
    let t = text(&o);
    assert!(
        t.contains("rollout 10%: this machine is outside the share"),
        "{t}"
    );
    assert!(t.contains("update takes it because you asked"), "{t}");
    assert_eq!(which_carton(&fx), "0.1.1");
}

#[test]
fn a6_an_update_inside_the_share_has_nothing_to_say_about_it() {
    let fx = installed();
    let id = id_where(|b| b < 10);
    let o = run(&fx, Some(&id), "10%", &["update"]);
    ok(&o);
    assert!(!text(&o).contains("rollout"), "{}", text(&o));
    assert_eq!(which_carton(&fx), "0.1.1");
}

// ---- no id, bad value ------------------------------------------------------------------------

#[test]
fn a6_a_machine_with_no_id_waits_below_100_and_is_in_at_100() {
    let fx = installed();
    let s = json(&run(&fx, None, "99%", &["--json", "status"]));
    let r = &s["data"]["rollout"];
    assert_eq!(
        (r["in_share"].clone(), r["reason"].clone()),
        (Value::Bool(false), Value::from("no-machine-id")),
        "{s}"
    );
    let (_, t) = on_tty(&fx, None, "99%", "list");
    assert!(t.contains("is rolling out"), "{t}");
    // 100 needs no id at all
    let s = json(&run(&fx, None, "100", &["--json", "status"]));
    assert_eq!(s["data"]["rollout"]["in_share"], true, "{s}");
    assert_eq!(s["data"]["rollout"]["reason"], Value::Null);
    // and the person's own update still works
    ok(&run(&fx, None, "50%", &["update"]));
    assert_eq!(which_carton(&fx), "0.1.1");
}

#[test]
fn a6_a_bad_rollout_value_waits_and_config_set_refuses_it() {
    let fx = installed();
    let id = id_where(|b| b < 5);
    for bad in ["banana", "0", "101", "10.5", "3x"] {
        let s = json(&run(&fx, Some(&id), bad, &["--json", "status"]));
        let r = &s["data"]["rollout"];
        assert_eq!(r["in_share"], false, "{bad}: {s}");
        assert_eq!(r["reason"], "invalid-setting", "{bad}: {s}");
        assert_eq!(r["percent"], Value::Null);
        let o = fx
            .cigi()
            .args(["--json", "config", "set", "rollout", bad])
            .output()
            .unwrap();
        assert_eq!(json(&o)["error"]["code"], "E1870", "{bad}: {}", text(&o));
        assert_ne!(o.status.code(), Some(0));
    }
    ok(&cigi(&fx, &["config", "set", "rollout", "25%"]));
    let c = cigi(&fx, &["--json", "config", "get", "rollout"]);
    ok(&c);
    assert_eq!(json(&c)["data"]["value"], "25%");
}

// ---- nothing leaves the machine --------------------------------------------------------------

fn files_under(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                files_under(&p, out);
            } else {
                out.push(p);
            }
        }
    }
}

#[test]
fn a6_no_output_report_file_or_request_carries_the_id_or_the_bucket() {
    let mut fx = fixture();
    two(&mut fx);
    let server = fx.serve_tls();
    let id = "tessa-rollout-canary-7f3a9c1e2b8d4605";
    let secret = [
        id.to_string(),
        digest_hex(id),
        digest_hex(id)[..16].to_string(),
    ];
    let mut outputs: Vec<String> = Vec::new();
    let mut go = |rollout: &str, args: &[&str]| {
        let o = fx
            .cigi_with_server(&server)
            .env("CIGI_ALLOW_SINGLE_SOURCE", "true")
            .env(ID_SEAM, id)
            .env("CIGI_ROLLOUT", rollout)
            .arg("--yes")
            .args(args)
            .output()
            .expect("run");
        let t = text(&o);
        outputs.push(t);
        o
    };
    ok(&go("100", &["install", "carton", "--carton", "0.1.0"]));
    for args in [
        &["--json", "status"][..],
        &["status"],
        &["--json", "update", "--check"],
        &["config"],
        &["--json", "config"],
        &["report"],
        &["--json", "report"],
        &["update"],
        &["--json", "status"],
        &["uninstall", "--dry-run"],
    ] {
        // some of these may refuse; what matters is that nothing they print carries the id
        go("40%", args);
    }
    let (_, tty) = on_tty(&fx, Some(id), "1%", "list");
    outputs.push(tty);

    for (i, t) in outputs.iter().enumerate() {
        for s in &secret {
            assert!(
                !t.contains(s.as_str()),
                "output {i} carries the id or its hash: {t}"
            );
        }
        assert!(
            !t.to_lowercase().contains("bucket"),
            "output {i} names the bucket: {t}"
        );
        assert!(!t.contains(ID_SEAM), "output {i} names the seam: {t}");
    }
    // every file the installer wrote (state, notice, journal, reports, config)
    let mut files = Vec::new();
    files_under(&fx.cigscript(), &mut files);
    files_under(&fx.user_home, &mut files);
    assert!(files.len() > 5, "{files:?}");
    for f in &files {
        let Ok(bytes) = std::fs::read(f) else {
            continue;
        };
        let body = String::from_utf8_lossy(&bytes).to_string();
        for s in &secret {
            assert!(
                !body.contains(s.as_str()),
                "{} carries the id or its hash",
                f.display()
            );
        }
    }
    // every request the run made
    let reqs = format!("{:?}", server.requests());
    assert!(reqs.len() > 10);
    for s in &secret {
        assert!(
            !reqs.contains(s.as_str()),
            "a request carries the id or its hash: {reqs}"
        );
    }
    server
        .assert_no_telemetry()
        .expect("no identifier in any request");
}

// ---- the inventory ---------------------------------------------------------------------------

#[test]
fn a6_status_json_is_the_inventory_and_makes_no_request() {
    let mut fx = fixture();
    two(&mut fx);
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    ok(&cigi(&fx, &["update"]));
    let server = fx.serve();
    server.clear();
    let id = id_where(|b| b >= 30);
    let o = fx
        .cigi_with_mirror(&server.url())
        .env("CIGI_ALLOW_SINGLE_SOURCE", "true")
        .env(ID_SEAM, &id)
        .env("CIGI_ROLLOUT", "30%")
        .args(["--json", "status"])
        .output()
        .unwrap();
    ok(&o);
    assert!(server.requests().is_empty(), "{:?}", server.paths());
    let d = json(&o)["data"].clone();
    assert_eq!(d["carton"], "0.1.1");
    assert_eq!(d["channel"], "stable");
    assert_eq!(d["tools"]["cig"], "1.21.0");
    assert_eq!(d["tools"]["cigo"], "1.17.0");
    let cartons: Vec<&str> = d["cartons"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c.as_str())
        .collect();
    assert!(
        cartons.contains(&"0.1.0") && cartons.contains(&"0.1.1"),
        "{cartons:?}"
    );
    // the share and whether this machine is in it: and nothing else about it
    let r = d["rollout"].as_object().unwrap();
    let mut keys: Vec<&str> = r.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["in_share", "percent", "reason"]);
    assert_eq!(d["rollout"]["percent"], 30);
    assert_eq!(d["rollout"]["in_share"], false);
    let all = String::from_utf8_lossy(&o.stdout).to_string();
    assert!(
        !all.contains(&id) && !all.contains(&digest_hex(&id)),
        "{all}"
    );
    // the human form asks for nothing either
    let o = fx
        .cigi_with_mirror(&server.url())
        .env(ID_SEAM, &id)
        .env("CIGI_ROLLOUT", "30%")
        .arg("status")
        .output()
        .unwrap();
    ok(&o);
    assert!(
        text(&o).contains("rollout 30%: this machine is not in the share yet"),
        "{}",
        text(&o)
    );
    assert!(server.requests().is_empty(), "{:?}", server.paths());
}

#[test]
fn a6_status_json_with_nothing_installed_still_carries_the_rollout() {
    let fx = fixture();
    let s = json(&run(&fx, Some("some-id"), "100", &["--json", "status"]));
    assert_eq!(s["data"]["carton"], Value::Null, "{s}");
    assert_eq!(s["data"]["rollout"]["in_share"], true);
    assert_eq!(s["data"]["cartons"], serde_json::json!([]));
}
