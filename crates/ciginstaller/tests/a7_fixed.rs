// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! Fixed-version announcements across the family (#12): a bug this machine reported against a
//! tool other than cig, whose fix the carton an update installs carries (by the manifest's
//! advisories), is told to its reporter once, in the issue's words.
//!
//! The products are the fixture's fakes: shell scripts, so these tests are Unix only.
#![cfg(unix)]

mod common;

use std::path::PathBuf;

use cigi_testkit::{sha256_hex, Fixture};
use common::{cigi, fixture, ok, text};
use serde_json::{json, Value};

const ISSUE: &str = "https://github.com/otmof-ops/cig-orchestrator/issues/12";

fn run(fx: &Fixture, args: &[&str]) -> std::process::Output {
    let mut c = fx.cigi();
    c.env("CIGI_ALLOW_SINGLE_SOURCE", "true").arg("--yes");
    c.args(args);
    cigi_testkit::output_retrying(&mut c).expect("run ciginstaller")
}

/// Carton 0.1.0 installed (cigo 1.17.0); 0.2.0 carries cigo 1.18.0. The advisory goes into the
/// manifest before anything reads it: a manifest that changes at the same sequence is refused.
fn two_cartons(id: &str, fixed_in: &str) -> Fixture {
    let mut fx = fixture();
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.17.0")]);
    fx.carton("0.2.0", &[("cig", "1.20.0"), ("cigo", "1.18.0")]);
    fx.publish("stable");
    advise_fix(&fx, id, fixed_in);
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    fx
}

/// The published manifest with an advisory naming `id`, fixed in `fixed_in`, affecting nothing
/// (so it is never shown to everyone), and its sidecar made to match.
fn advise_fix(fx: &Fixture, id: &str, fixed_in: &str) {
    let path = fx.releases.join("manifest-stable.toml");
    let mut t = std::fs::read_to_string(&path).expect("manifest");
    t.push_str(&format!(
        "\n[[advisory]]\nid = \"{id}\"\naffects = []\nfixed_in = \"{fixed_in}\"\nseverity = \"bug\"\n\
         summary = \"rollback skipped a file after a box apply\"\nurl = \"{ISSUE}\"\n"
    ));
    std::fs::write(&path, &t).expect("write manifest");
    std::fs::write(
        fx.releases.join("manifest-stable.toml.sha256"),
        format!("{}  manifest-stable.toml\n", sha256_hex(t.as_bytes())),
    )
    .expect("write sidecar");
}

/// A report this machine filed against `repo` as issue #12.
fn filed_report(fx: &Fixture, origin: &str, repo: &str) -> PathBuf {
    let dir = fx.cigscript().join("ashtray");
    std::fs::create_dir_all(&dir).expect("ashtray");
    let id = format!("20261111-120000-{origin}");
    let r = json!({
        "id": id,
        "fingerprint": "f00d",
        "version": "1.17.0",
        "os": "linux",
        "arch": "x86_64",
        "timestamp": "2026-11-11T12:00:00Z",
        "command": [origin, "run", "deploy"],
        "cwd": "<project>",
        "panic_message": "rollback skipped a file after a box apply",
        "panic_location": "<unknown>",
        "backtrace": [],
        "origin": origin,
        "repo": repo,
        "title": "rollback skipped a file after a box apply",
        "sent": "2026-11-11T12:01:00Z",
        "issue": { "repo": repo, "number": 12, "url": format!("https://github.com/{repo}/issues/12") },
    });
    let p = dir.join(format!("{id}.json"));
    std::fs::write(&p, serde_json::to_string_pretty(&r).unwrap()).expect("write report");
    p
}

fn read(p: &PathBuf) -> Value {
    serde_json::from_str(&std::fs::read_to_string(p).expect("read report")).expect("json")
}

#[test]
fn an_update_that_installs_the_fix_tells_the_reporter_once() {
    let fx = two_cartons("otmof-ops/cig-orchestrator#12", "1.18.0");
    let report = filed_report(&fx, "cigo", "otmof-ops/cig-orchestrator");
    let o = run(&fx, &["update", "--no-trial"]);
    ok(&o);
    let t = text(&o);
    assert!(
        t.contains(
            "the bug you reported (cig-orchestrator#12, rollback skipped a file after a box apply) \
             is fixed in cigo 1.18.0, which this update installed with carton 0.2.0"
        ),
        "{t}"
    );
    let r = read(&report);
    assert_eq!(r["announced"], true, "{r}");
    assert_eq!(r["fixed_in"], "1.18.0", "{r}");
    // told once: the report stays quiet after that
    assert_eq!(t.matches("the bug you reported").count(), 1, "{t}");
}

#[test]
fn an_advisory_found_by_its_issue_url_counts_as_well() {
    let fx = two_cartons("CIGO-2026-0012", "1.18.0");
    let report = filed_report(&fx, "cigo", "otmof-ops/cig-orchestrator");
    let o = run(&fx, &["update", "--no-trial"]);
    ok(&o);
    assert!(text(&o).contains("is fixed in cigo 1.18.0"), "{}", text(&o));
    assert_eq!(read(&report)["announced"], true);
}

#[test]
fn a_fix_in_a_later_build_than_the_carton_carries_is_not_told() {
    let fx = two_cartons("otmof-ops/cig-orchestrator#12", "1.19.0");
    let report = filed_report(&fx, "cigo", "otmof-ops/cig-orchestrator");
    let o = run(&fx, &["update", "--no-trial"]);
    ok(&o);
    assert!(!text(&o).contains("the bug you reported"), "{}", text(&o));
    assert!(read(&report).get("announced").is_none_or(|a| a == false));
}

#[test]
fn cigs_own_reports_are_left_to_cig() {
    let fx = two_cartons("otmof-ops/cig-script#12", "1.20.0");
    let report = filed_report(&fx, "cig", "otmof-ops/cig-script");
    let o = run(&fx, &["update", "--no-trial"]);
    ok(&o);
    assert!(
        !text(&o).contains("the bug you reported (cig-script#12"),
        "{}",
        text(&o)
    );
    assert!(read(&report).get("announced").is_none_or(|a| a == false));
}
