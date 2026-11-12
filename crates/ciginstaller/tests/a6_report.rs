// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `ciginstaller report` (#11): the version block with a mismatch flagged, each tool's build in
//! it (#3), the bundle with the redacted journal and the trial's differences, cig's own bundle
//! for a cig run, `--out`, and `--send` through round 1's route and consent with no identifiers
//! (#19).
//!
//! #11's done-when: a report carries the version block, and a deliberately mismatched
//! combination is flagged in it. The products are the fixture's fakes (shell scripts), so these
//! tests are Unix only; the fake cig answers the bundle call.
#![cfg(unix)]

mod common;

use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::{Arc, Mutex};

use cigi_testkit::Fixture;
use common::{cigi, fixture, ok, text};
use serde_json::{json, Value};

const USER: &str = "tessa-q";
const HOST: &str = "bunnyhutch";

// ---- the fakes -----------------------------------------------------------------------------

/// A cig that answers `report-bundle`, with a person's name and folder in what it says (cig
/// redacts its own; the installer redacts again, and this proves the second pass).
fn bundle_answer(fx: &Fixture) -> String {
    let home = fx.user_home.to_string_lossy().to_string();
    let body = json!({
        "kind": "cigscript-report",
        "cig": "1.20.0",
        "run": {"id": "run-77", "status": "failed", "error": format!("cannot read {home}/projects/{USER}-notes/plan.txt on {HOST}")},
        "journal": [{"seq": 1, "op": "exhale", "path": format!("{home}/projects/{USER}-notes/plan.txt")}],
        "diagnostic": {"code": "E501"},
    });
    body.to_string()
}

/// cig 1.20.0 with `report-bundle`; calls are logged to `$HOME/cig-calls`.
fn cig_with_bundle(fx: &mut Fixture) {
    let answer = bundle_answer(fx);
    fx.product("cig", "1.20.0").dry_run_output("plan: 1 op").shell(&format!(
        "echo \"$*\" >> \"$HOME/cig-calls\"\nif [ \"$1\" = \"report-bundle\" ]; then if [ \"$2\" = \"run-77\" ]; then printf '%s' '{}'; exit 0; fi; echo 'error[E803 usage]: no run matches' >&2; exit 3; fi\n",
        answer.replace('\'', "'\\''")
    ));
}

/// The cig on main today: no `report-bundle`, but `cig --json report <run>`.
fn cig_without_bundle(fx: &mut Fixture) {
    let answer = bundle_answer(fx);
    fx.product("cig", "1.20.0").dry_run_output("plan: 1 op").shell(&format!(
        "echo \"$*\" >> \"$HOME/cig-calls\"\nif [ \"$1\" = \"report-bundle\" ]; then echo \"error: unrecognized subcommand 'report-bundle'\" >&2; exit 2; fi\nif [ \"$1 $2\" = \"--json report\" ]; then if [ \"$3\" = \"run-77\" ]; then printf '%s' '{}'; exit 0; fi; echo 'error[E803 usage]: no run matches' >&2; exit 3; fi\n",
        answer.replace('\'', "'\\''")
    ));
}

/// Two cartons: 0.1.0 (cig 1.20.0, cigo 1.17.0) and 0.2.0 (cig 1.21.0, cigo 1.18.0, plus the
/// other two tools), installed on 0.1.0.
fn two_cartons(fx: &mut Fixture, new_plan: &str) {
    fx.product("cig", "1.21.0").dry_run_output(new_plan);
    fx.carton(
        "0.1.0",
        &[
            ("cig", "1.20.0"),
            ("cigo", "1.17.0"),
            ("cigcompile", "0.2.0"),
            ("cigar", "0.1.0"),
        ],
    );
    fx.carton(
        "0.2.0",
        &[
            ("cig", "1.21.0"),
            ("cigo", "1.18.0"),
            ("cigcompile", "0.2.0"),
            ("cigar", "0.1.0"),
        ],
    );
    fx.publish("stable");
}

fn installed_one(mut fx: Fixture) -> Fixture {
    two_cartons(&mut fx, "plan: 1 op");
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    fx
}

fn run(fx: &Fixture, env: &[(&str, &str)], args: &[&str]) -> Output {
    let mut c = fx.cigi();
    c.env("CIGI_ALLOW_SINGLE_SOURCE", "true")
        .env("USER", USER)
        .env("HOSTNAME", HOST);
    for (k, v) in env {
        c.env(k, v);
    }
    c.args(args);
    cigi_testkit::output_retrying(&mut c).expect("run ciginstaller")
}

fn json_of(o: &Output) -> Value {
    let line = String::from_utf8_lossy(&o.stdout)
        .lines()
        .last()
        .unwrap_or("")
        .to_string();
    serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {}", text(o)))
}

fn data(fx: &Fixture, args: &[&str]) -> Value {
    data_with(fx, &[], args)
}

fn data_with(fx: &Fixture, env: &[(&str, &str)], args: &[&str]) -> Value {
    let mut a = vec!["--json"];
    a.extend_from_slice(args);
    let o = run(fx, env, &a);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    json_of(&o)["data"].clone()
}

fn mismatches(block: &Value) -> Vec<String> {
    block["mismatches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["text"].as_str().unwrap().to_string())
        .collect()
}

fn tool<'a>(block: &'a Value, name: &str) -> &'a Value {
    block["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["tool"] == name)
        .unwrap_or_else(|| panic!("no {name} in {block:#}"))
}

// ---- the fake route ------------------------------------------------------------------------

/// A request the fake route saw: its headers and its body.
type Seen = (Vec<(String, String)>, String);

struct Route {
    base: String,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Route {
    fn start() -> Route {
        let server = tiny_http::Server::http("127.0.0.1:0").expect("bind a fake route");
        let port = server.server_addr().to_ip().expect("ip").port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        std::thread::spawn(move || {
            for mut req in server.incoming_requests() {
                let mut body = String::new();
                let _ = std::io::Read::read_to_string(req.as_reader(), &mut body);
                let headers = req
                    .headers()
                    .iter()
                    .map(|h| {
                        (
                            h.field.to_string().to_ascii_lowercase(),
                            h.value.to_string(),
                        )
                    })
                    .collect();
                log.lock().unwrap().push((headers, body));
                let r = tiny_http::Response::from_string(
                    r#"{"html_url":"https://github.com/otmof-ops/cig-installer/issues/52"}"#,
                )
                .with_status_code(201);
                let _ = req.respond(r);
            }
        });
        Route {
            base: format!("http://127.0.0.1:{port}/reports"),
            seen,
        }
    }
    fn requests(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

fn schema_keys() -> Vec<String> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schemas/report.schema.json");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
    v["properties"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect()
}

fn ashtray_files(fx: &Fixture) -> Vec<PathBuf> {
    std::fs::read_dir(fx.cigscript().join("ashtray"))
        .map(|d| {
            d.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "json"))
                .collect()
        })
        .unwrap_or_default()
}

// ---- #11's done-when -----------------------------------------------------------------------

#[test]
fn a_report_carries_the_version_block() {
    let fx = installed_one(fixture());
    let d = data(&fx, &["report", "--block"]);
    assert_eq!(d["format"], 1);
    assert_eq!(d["installer"]["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(d["carton"]["machine"], "0.1.0");
    assert_eq!(d["platform"]["os"], std::env::consts::OS);
    for (t, v) in [
        ("cig", "1.20.0"),
        ("cigo", "1.17.0"),
        ("cigcompile", "0.2.0"),
        ("cigar", "0.1.0"),
    ] {
        assert_eq!(tool(&d, t)["version"], v);
    }
    assert_eq!(d["guarantee"]["inside"], true, "{d:#}");
    assert_eq!(d["guarantee"]["carton"], "0.1.0");
    assert!(mismatches(&d).is_empty(), "{d:#}");
    // and the bundle a person sends has the same block in it
    let b = data(&fx, &["report"]);
    assert_eq!(b["bundle"]["version_block"]["carton"]["machine"], "0.1.0");
    assert_eq!(b["bundle"]["kind"], "ciginstaller-report");
}

#[test]
fn a_deliberately_mismatched_combination_is_flagged_in_it() {
    let fx = installed_one(fixture());
    ok(&cigi(&fx, &["update", "--no-trial"]));
    // cig goes back to its older build while cigo stays on the new carton's: a mix no carton holds
    ok(&cigi(&fx, &["rollback", "cig"]));
    let d = data(&fx, &["report", "--block"]);
    assert_eq!(tool(&d, "cig")["version"], "1.20.0");
    assert_eq!(tool(&d, "cig")["state"], "rolled-back");
    assert_eq!(tool(&d, "cigo")["version"], "1.18.0");
    let m = mismatches(&d);
    assert!(
        m.contains(&"cigo 1.18.0 with cig 1.20.0: not from the same carton".to_string()),
        "{m:?}"
    );
    assert_eq!(d["guarantee"]["inside"], false);
    assert_eq!(d["guarantee"]["carton"], Value::Null);
    // a person sees it too, in words
    let o = run(&fx, &[], &["report", "--block"]);
    assert!(
        text(&o).contains("flagged: cigo 1.18.0 with cig 1.20.0: not from the same carton"),
        "{}",
        text(&o)
    );
    assert!(
        text(&o).contains("NOT inside the manifest's guarantees"),
        "{}",
        text(&o)
    );
    // the bundle carries the same flag
    let b = data(&fx, &["report"]);
    assert!(mismatches(&b["bundle"]["version_block"]).len() == m.len());
}

#[test]
fn a_project_pin_is_shown_as_versions_and_as_what_the_project_runs() {
    let fx = installed_one(fixture());
    ok(&cigi(&fx, &["update", "--no-trial"]));
    // the project pins cig to the older build: the resolver runs the whole of carton 0.1.0 there,
    // so the project differs from the machine (shown) and is still one carton (not flagged)
    std::fs::write(fx.project.join(".cig-version"), "1.20.0\n").unwrap();
    let d = data(&fx, &["report", "--block"]);
    let p = &d["project"];
    assert_eq!(p["legacy_pins"][0]["pin"], "1.20.0", "{d:#}");
    assert_eq!(p["legacy_pins"][0]["file"], ".cig-version");
    assert_eq!(d["carton"]["machine"], "0.2.0");
    assert_eq!(tool(&d, "cig")["version"], "1.21.0");
    assert_eq!(tool(&d, "cig")["runs_in_project"], "1.20.0");
    assert_eq!(tool(&d, "cigo")["runs_in_project"], "1.17.0");
    assert!(mismatches(&d).is_empty(), "{d:#}");
    assert_eq!(d["guarantee"]["inside"], true);
    // pins are versions and file names, never paths
    assert!(
        !d.to_string().contains(fx.project.to_str().unwrap()),
        "{d:#}"
    );
}

// ---- #3: each tool's build -----------------------------------------------------------------

#[test]
fn every_tools_build_is_in_the_report() {
    let fx = installed_one(fixture());
    let d = data(&fx, &["report", "--block"]);
    for t in ["cig", "cigo", "cigcompile", "cigar"] {
        let row = tool(&d, t);
        let build = row["build"].as_str().unwrap();
        assert!(
            build.starts_with("sha256:") && build.len() == 7 + 12,
            "{t}: {build}"
        );
        assert!(build[7..].bytes().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(row["trust"], "checksummed");
        assert_eq!(row["from_carton"], "0.1.0");
    }
    // two builds of one tool are told apart by it
    let o = run(&fx, &[], &["report", "--block"]);
    assert!(text(&o).contains("cig 1.20.0 (sha256:"), "{}", text(&o));
}

// ---- the bundle: journal, trial, metadata --------------------------------------------------

#[test]
fn the_bundle_has_the_redacted_journal_the_trials_differences_and_the_metadata_versions() {
    let mut fx = fixture();
    two_cartons(&mut fx, "plan: 2 ops");
    fx.cigscript();
    std::fs::write(fx.project.join("a.cig"), "exhale \"a\"\n").unwrap();
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    // the trial finds a difference and, with no terminal, the update stays on 0.1.0
    let o = run(&fx, &[], &["update"]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(text(&o).contains("E1851"), "{}", text(&o));

    let d = data(&fx, &["report"]);
    let b = &d["bundle"];
    let op = &b["operation"];
    assert_eq!(op["status"], "rolled-back", "{op:#}");
    assert!(
        op["label"]
            .as_str()
            .unwrap()
            .contains("update carton 0.1.0 to 0.2.0"),
        "{op:#}"
    );
    assert!(op["journal_lines"].as_u64().unwrap() >= 2, "{op:#}");
    assert!(op["journal"]
        .as_array()
        .unwrap()
        .iter()
        .all(|l| l.get("pid").is_none()));
    // the trial's differences travel
    let t = &op["trial"];
    assert_eq!(t["compared"], 1, "{op:#}");
    assert_eq!(t["different"].as_array().unwrap().len(), 1);
    assert_eq!(t["different"][0]["script"], "a.cig");
    assert!(t["different"][0]["diff"]
        .as_str()
        .unwrap()
        .contains("plan: 2 ops"));
    // the metadata versions it saw
    let md = &b["version_block"]["metadata"];
    assert!(md["manifest"]["sequence"].as_u64().is_some(), "{md:#}");
    assert_eq!(md["manifest"]["channel"], "stable");
    assert_eq!(md["journal_format"], 1);
    assert_eq!(md["state_format"], 1);
    assert!(md["receipt_sequences"]["cig"].as_u64().is_some(), "{md:#}");
    // none of the machine is in it
    let all = b.to_string();
    for secret in [
        USER,
        HOST,
        fx.user_home.to_str().unwrap(),
        fx.project.to_str().unwrap(),
    ] {
        assert!(!all.contains(secret), "{secret} is in the bundle:\n{all}");
    }
}

#[test]
fn a_run_is_found_by_its_history_id_or_its_transaction_id() {
    let fx = installed_one(fixture());
    let h = data(&fx, &["history"]);
    let id = h["entries"][0]["id"].as_str().unwrap().to_string();
    let txn = h["entries"][0]["txn"].as_str().unwrap().to_string();
    for key in [&id, &txn[..12.min(txn.len())].to_string()] {
        let d = data(&fx, &["report", key]);
        assert_eq!(d["bundle"]["operation"]["id"], txn.as_str(), "{key}");
        assert_eq!(d["bundle"]["operation"]["history"]["what"], "install");
    }
}

// ---- a cig run: cig's own bundle -----------------------------------------------------------

fn cig_fixture(builder: fn(&mut Fixture)) -> Fixture {
    let mut fx = fixture();
    builder(&mut fx);
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.17.0")]);
    fx.publish("stable");
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    fx
}

fn calls(fx: &Fixture) -> Vec<String> {
    std::fs::read_to_string(fx.user_home.join("cig-calls"))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn a_cig_run_is_bundled_by_cig_through_report_bundle() {
    let fx = cig_fixture(cig_with_bundle);
    let d = data(&fx, &["report", "run-77"]);
    let b = &d["bundle"];
    assert_eq!(b["cig_bundle"]["kind"], "cigscript-report");
    assert_eq!(b["cig_bundle"]["diagnostic"]["code"], "E501");
    assert_eq!(b["run"], "run-77");
    assert_eq!(calls(&fx), ["report-bundle run-77 --json"]);
    // cig redacts its own; the installer's second pass leaves nothing of the person
    let all = b.to_string();
    for secret in [USER, HOST, fx.user_home.to_str().unwrap()] {
        assert!(!all.contains(secret), "{secret} in {all}");
    }
}

#[test]
fn the_cig_on_main_has_no_report_bundle_so_json_report_is_the_fallback() {
    let fx = cig_fixture(cig_without_bundle);
    let d = data(&fx, &["report", "--cig", "run-77"]);
    assert_eq!(d["bundle"]["cig_bundle"]["kind"], "cigscript-report");
    assert_eq!(
        calls(&fx),
        ["report-bundle run-77 --json", "--json report run-77"]
    );
}

#[test]
fn a_run_nobody_has_is_a_usage_error_and_a_flag_is_not_a_run() {
    let fx = cig_fixture(cig_with_bundle);
    let o = run(&fx, &[], &["report", "no-such-run"]);
    assert_eq!(o.status.code(), Some(3), "{}", text(&o));
    assert!(text(&o).contains("E1870"), "{}", text(&o));
    let o = run(&fx, &[], &["report", "--cig", "--", "-x"]);
    assert_eq!(o.status.code(), Some(3), "{}", text(&o));
    assert_eq!(
        calls(&fx),
        ["report-bundle no-such-run --json"],
        "cig was asked once, and not for `-x`"
    );
}

#[test]
fn a_cig_that_gives_no_bundle_is_e1858() {
    let mut fx = fixture();
    fx.product("cig", "1.20.0")
        .shell("echo 'it fell over' >&2\nexit 1\n");
    fx.carton("0.1.0", &[("cig", "1.20.0")]);
    fx.publish("stable");
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    let o = run(&fx, &[], &["report", "--cig", "run-1"]);
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
    assert!(text(&o).contains("E1858"), "{}", text(&o));
    assert!(text(&o).contains("it fell over"), "{}", text(&o));
}

// ---- --out ---------------------------------------------------------------------------------

#[test]
fn out_writes_the_bundle_and_nothing_is_written_without_it() {
    let fx = installed_one(fixture());
    let before = ashtray_files(&fx).len();
    let o = run(&fx, &[], &["report"]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    assert!(
        text(&o).contains("nothing is written or sent until you ask"),
        "{}",
        text(&o)
    );
    let out = fx.user_home.join("out").join("report.json");
    let o = run(
        &fx,
        &[],
        &["--json", "report", "--out", out.to_str().unwrap()],
    );
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    let b: Value = serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    assert_eq!(b["kind"], "ciginstaller-report");
    assert_eq!(b["version_block"]["carton"]["machine"], "0.1.0");
    assert_eq!(
        ashtray_files(&fx).len(),
        before,
        "--out does not touch the ashtray"
    );
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&out).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let all = std::fs::read_to_string(&out).unwrap();
    for secret in [USER, HOST, fx.user_home.to_str().unwrap()] {
        assert!(!all.contains(secret), "{secret} in {all}");
    }
    // a folder that cannot be written is E1858
    let o = run(&fx, &[], &["report", "--out", "/proc/nope/report.json"]);
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
    assert!(text(&o).contains("E1858"), "{}", text(&o));
}

// ---- --send: round 1's route and consent, no identifiers -----------------------------------

#[test]
fn send_goes_through_the_route_as_a_report_the_schema_allows_with_no_identifier() {
    let fx = cig_fixture(cig_with_bundle);
    let route = Route::start();
    let o = run(
        &fx,
        &[("CIGI_REPORT_ROUTE", &route.base)],
        &["report", "run-77", "--send"],
    );
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    assert!(text(&o).contains("issues/52"), "{}", text(&o));
    let reqs = route.requests();
    assert_eq!(reqs.len(), 1, "one POST");
    let (headers, body) = &reqs[0];
    let v: Value = serde_json::from_str(body).unwrap();
    // it passes round 1's schema: every key is an allowed property, the required ones are there
    let allowed = schema_keys();
    for k in v.as_object().unwrap().keys() {
        assert!(allowed.contains(k), "{k} is not in the schema");
    }
    for k in [
        "id",
        "fingerprint",
        "kind",
        "origin",
        "version",
        "os",
        "arch",
        "timestamp",
        "command",
        "cwd",
        "panic_message",
        "panic_location",
        "backtrace",
    ] {
        assert!(!v[k].is_null(), "{k} missing: {v}");
    }
    assert_eq!(v["kind"], "report");
    assert_eq!(v["origin"], "ciginstaller");
    assert_eq!(v["fingerprint"].as_str().unwrap().len(), 8);
    // the block and cig's bundle are in it
    assert_eq!(v["details"]["version_block"]["carton"]["machine"], "0.1.0");
    assert_eq!(v["details"]["cig_bundle"]["kind"], "cigscript-report");
    // #19: no user, host, folder, token or cookie anywhere in what left
    for secret in [
        USER,
        HOST,
        fx.user_home.to_str().unwrap(),
        fx.cigscript().to_str().unwrap(),
    ] {
        assert!(!body.contains(secret), "{secret} left the machine:\n{body}");
    }
    for (k, _) in headers {
        assert!(
            !matches!(
                k.as_str(),
                "authorization" | "cookie" | "proxy-authorization"
            ),
            "{k}"
        );
    }
    // the ashtray keeps it as filed
    let f = ashtray_files(&fx);
    assert_eq!(f.len(), 1);
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(&f[0]).unwrap()).unwrap();
    assert_eq!(saved["kind"], "report");
    assert!(saved["issue"]["url"]
        .as_str()
        .unwrap()
        .ends_with("/issues/52"));
}

#[test]
fn what_out_writes_is_what_send_carries() {
    let fx = installed_one(fixture());
    let route = Route::start();
    let out = fx.user_home.join("r.json");
    ok(&run(&fx, &[], &["report", "--out", out.to_str().unwrap()]));
    ok(&run(
        &fx,
        &[("CIGI_REPORT_ROUTE", &route.base)],
        &["report", "--send"],
    ));
    let written: Value = serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    let sent: Value = serde_json::from_str(&route.requests()[0].1).unwrap();
    assert_eq!(written["version_block"], sent["details"]["version_block"]);
}

#[test]
fn send_obeys_never_and_keeps_to_the_queue_without_a_route() {
    let fx = installed_one(fixture());
    let route = Route::start();
    // never: refused before anything is saved, however it is set
    for env in [("CIG_REPORTS", "never"), ("CIGI_REPORTS", "never")] {
        let o = run(
            &fx,
            &[env, ("CIGI_REPORT_ROUTE", &route.base)],
            &["report", "--send"],
        );
        assert_eq!(o.status.code(), Some(2), "{}", text(&o));
        assert!(text(&o).contains("E1857"), "{}", text(&o));
    }
    assert!(route.requests().is_empty());
    assert!(
        ashtray_files(&fx).is_empty(),
        "a refused report is not saved"
    );
    // no route yet: the report waits and says so, exit 0
    let o = run(&fx, &[], &["report", "--send"]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    assert!(text(&o).contains("no issues route yet"), "{}", text(&o));
    assert_eq!(ashtray_files(&fx).len(), 1);
    // --out and --send are one or the other, and --block takes neither
    for args in [
        &["report", "--out", "x", "--send"][..],
        &["report", "--block", "--send"],
        &["report", "--block", "--out", "x"],
    ] {
        assert_eq!(run(&fx, &[], args).status.code(), Some(3), "{args:?}");
    }
}

// ---- a broken install ----------------------------------------------------------------------

#[test]
fn a_broken_state_still_gives_a_block_that_says_so() {
    let fx = installed_one(fixture());
    std::fs::write(fx.cigscript().join("installer/state.json"), "{ not json").unwrap();
    let d = data(&fx, &["report", "--block"]);
    assert_eq!(d["installer"]["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(d["carton"]["machine"], Value::Null);
    assert_eq!(d["mismatches"][0]["kind"], "state");
    assert_eq!(d["guarantee"]["inside"], Value::Null);
    // and a report on it can still be written
    let out = fx.user_home.join("broken.json");
    ok(&run(&fx, &[], &["report", "--out", out.to_str().unwrap()]));
    assert!(out.is_file());
}

// ---- the block on a report another tool queued (#11) ---------------------------------------

/// A report as cig or cigo writes it to the ashtray: it knows its own version and nothing of
/// the carton.
fn queued_by(fx: &Fixture, origin: &str, version: &str, id: &str) {
    let dir = fx.cigscript().join("ashtray");
    std::fs::create_dir_all(&dir).unwrap();
    let r = json!({
        "id": id, "fingerprint": "abc12345", "version": version, "os": "linux", "arch": "x86_64",
        "timestamp": "2026-11-09T10:15:00Z", "command": [origin, "run", "<script>"], "cwd": "~/d1",
        "panic_message": "index out of range", "panic_location": "src/run.rs:1:1", "backtrace": [],
        "kind": "crash", "origin": origin
    });
    std::fs::write(dir.join(format!("{id}.json")), r.to_string()).unwrap();
}

/// `crash show` and `crash send --via file` of one report: the object each makes.
fn shown_and_sent(fx: &Fixture, id: &str) -> (Value, Value) {
    shown_and_sent_with(fx, &[], id)
}

fn shown_and_sent_with(fx: &Fixture, env: &[(&str, &str)], id: &str) -> (Value, Value) {
    let shown = data_with(fx, env, &["crash", "show", id])["report"].clone();
    let out = fx.user_home.join(format!("{id}.export.json"));
    let o = run(
        fx,
        env,
        &[
            "--json",
            "crash",
            "send",
            id,
            "--via",
            "file",
            "--out",
            out.to_str().unwrap(),
        ],
    );
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    let sent: Value = serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    (shown, sent["report"].clone())
}

#[test]
fn a_report_each_tool_queued_carries_the_version_block_when_it_is_sent() {
    let fx = installed_one(fixture());
    let block = data(&fx, &["report", "--block"]);
    queued_by(&fx, "cig", "1.20.0", "20261109T101500-cig001");
    queued_by(&fx, "cigo", "1.17.0", "20261109T101600-cigo01");
    for (id, origin) in [
        ("20261109T101500-cig001", "cig"),
        ("20261109T101600-cigo01", "cigo"),
    ] {
        let (shown, sent) = shown_and_sent(&fx, id);
        assert_eq!(shown, sent, "show and send are the same object ({origin})");
        assert_eq!(shown["origin"], origin);
        let got = &shown["details"]["version_matrix"];
        assert_eq!(got["taken"], "at send time", "{origin}: {shown:#}");
        let mut plain = got.clone();
        plain.as_object_mut().unwrap().remove("taken");
        assert_eq!(plain, block, "the block `report --block` prints ({origin})");
        assert_eq!(tool(got, "cig")["version"], "1.20.0");
        assert_eq!(tool(got, "cigo")["version"], "1.17.0");
        // nothing identifying the person or the machine rides in it
        let all = shown.to_string();
        assert!(!all.contains(USER) && !all.contains(HOST), "{all}");
    }
    // the saved report is untouched: the block is taken at send time, never stored
    let saved = std::fs::read_to_string(fx.cigscript().join("ashtray/20261109T101500-cig001.json"))
        .unwrap();
    assert!(!saved.contains("version_matrix"), "{saved}");
}

/// Host names to try against this machine's kernel version: every run of three or more letters
/// in it, whether it is a whole part of the version (`oem` in `6.17.0-1033-oem`) or inside a
/// longer one (`arch` in `6.11.2-arch1-1`), and one that is in no kernel's version.
fn host_names_for_this_kernel() -> Vec<String> {
    let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default();
    let mut names: Vec<String> = kernel
        .trim()
        .split(|c: char| !c.is_ascii_alphabetic())
        .filter(|w| w.len() >= 3 && !matches!(*w, "root" | "user" | "home"))
        .map(str::to_string)
        .collect();
    names.dedup();
    names.push(HOST.to_string());
    names
}

#[test]
fn report_block_prints_what_a_queued_report_carries_whatever_the_host_is_named() {
    let fx = installed_one(fixture());
    for (n, name) in host_names_for_this_kernel().iter().enumerate() {
        let env = [("HOSTNAME", name.as_str())];
        let block = data_with(&fx, &env, &["report", "--block"]);
        let id = format!("20261109T1015{n:02}-cig003");
        queued_by(&fx, "cig", "1.20.0", &id);
        let (shown, sent) = shown_and_sent_with(&fx, &env, &id);
        assert_eq!(shown, sent);
        let got = &shown["details"]["version_matrix"];
        assert_eq!(got["platform"], block["platform"], "host {name}: {got:#}");
        let mut plain = got.clone();
        plain.as_object_mut().unwrap().remove("taken");
        assert_eq!(plain, block, "host {name}");
        // the block printed is the one that travels: the name is not in it
        let printed = block.to_string();
        let whole = printed
            .split(|c: char| !c.is_alphanumeric())
            .any(|w| w == name);
        assert!(!whole, "host {name} in {printed}");
    }
}

#[test]
fn a_block_a_product_embeds_from_report_block_travels_unchanged() {
    let fx = installed_one(fixture());
    let block = data(&fx, &["report", "--block"]);
    let dir = fx.cigscript().join("ashtray");
    std::fs::create_dir_all(&dir).unwrap();
    let id = "20261109T101800-own002";
    let r = json!({
        "id": id, "fingerprint": "abc12345", "version": "1.20.0", "os": "linux",
        "arch": "x86_64", "timestamp": "2026-11-09T10:18:00Z", "command": [], "cwd": "", "panic_message": "x",
        "panic_location": "", "backtrace": [], "kind": "crash", "origin": "cig",
        "details": {"version_matrix": block.clone()}
    });
    std::fs::write(dir.join(format!("{id}.json")), r.to_string()).unwrap();
    let (shown, sent) = shown_and_sent(&fx, id);
    assert_eq!(shown, sent);
    assert_eq!(shown["details"]["version_matrix"], block);
}

#[test]
fn a_host_name_standing_on_its_own_still_leaves_the_report() {
    let fx = installed_one(fixture());
    let dir = fx.cigscript().join("ashtray");
    std::fs::create_dir_all(&dir).unwrap();
    let id = "20261109T101500-cig004";
    let r = json!({
        "id": id, "fingerprint": "abc12345", "version": "1.20.0", "os": "linux", "arch": "x86_64",
        "timestamp": "2026-11-09T10:15:00Z", "command": ["cig", "run", "<script>"], "cwd": "~/d1",
        "panic_message": format!("lost {USER}@{HOST} on {HOST}: see /srv/{HOST}/log ({USER})"),
        "panic_location": "src/run.rs:1:1", "backtrace": [], "kind": "crash", "origin": "cig"
    });
    std::fs::write(dir.join(format!("{id}.json")), r.to_string()).unwrap();
    let (shown, _) = shown_and_sent(&fx, id);
    let all = shown.to_string();
    assert!(!all.contains(USER) && !all.contains(HOST), "{all}");
    assert!(all.contains("<user>@<host>"), "{all}");
}

#[test]
fn a_mismatched_carton_is_flagged_in_the_block_on_a_tools_report() {
    let fx = installed_one(fixture());
    ok(&cigi(&fx, &["update", "--no-trial"]));
    ok(&cigi(&fx, &["rollback", "cig"]));
    queued_by(&fx, "cigo", "1.18.0", "20261109T101600-cigo02");
    let (shown, sent) = shown_and_sent(&fx, "20261109T101600-cigo02");
    assert_eq!(shown, sent);
    let m = mismatches(&shown["details"]["version_matrix"]);
    assert!(
        m.contains(&"cigo 1.18.0 with cig 1.20.0: not from the same carton".to_string()),
        "{m:?}"
    );
    assert_eq!(
        shown["details"]["version_matrix"]["guarantee"]["inside"],
        false
    );
}

#[test]
fn a_block_the_report_already_carries_is_left_alone() {
    let fx = installed_one(fixture());
    let dir = fx.cigscript().join("ashtray");
    std::fs::create_dir_all(&dir).unwrap();
    let r = json!({
        "id": "20261109T101700-own001", "fingerprint": "abc12345", "version": "1.20.0", "os": "linux",
        "arch": "x86_64", "timestamp": "2026-11-09T10:17:00Z", "command": [], "cwd": "", "panic_message": "x",
        "panic_location": "", "backtrace": [], "kind": "crash", "origin": "cig",
        "details": {"version_matrix": {"cig": "1.20.0"}}
    });
    std::fs::write(dir.join("20261109T101700-own001.json"), r.to_string()).unwrap();
    let (shown, sent) = shown_and_sent(&fx, "20261109T101700-own001");
    assert_eq!(shown, sent);
    assert_eq!(shown["details"]["version_matrix"], json!({"cig": "1.20.0"}));
}
