// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! The report routes prove themselves (#28 steps 2 and 4): every route against a fake tracker
//! (including its 422, 403, rate limit and redirect), blame routing, the `cig-report v1` block
//! read back the way the hive reads it, and fault injection that ends with the report filed
//! exactly once or still queued with the reason. No test reaches the real network: the tracker
//! is on 127.0.0.1 and `gh` is a script.

mod common;

#[cfg(unix)]
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use cigi_testkit::{Endpoint, FakeTracker, Fault, Fixture};
use serde_json::{json, Value};

const TOKEN: &str = "tkn-fake-0123456789";
const CIG: &str = "otmof-ops/cig-script";
const CIGO: &str = "otmof-ops/cig-orchestrator";

/// Run with a hard stop: a hung child is killed and the test fails.
fn run(mut c: Command) -> Output {
    c.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = c.spawn().expect("start ciginstaller");
    let pid = child.id();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    match rx.recv_timeout(Duration::from_secs(90)) {
        Ok(Ok(o)) => o,
        _ => {
            let _ = Command::new("kill").args(["-9", &pid.to_string()]).status();
            panic!("ciginstaller did not finish in 90 seconds");
        }
    }
}

fn cigi(fx: &Fixture, env: &[(&str, &str)], args: &[&str]) -> Output {
    let mut c = fx.cigi();
    c.env("USER", "tessa-q").env("HOSTNAME", "bunnyhutch");
    for (k, v) in env {
        c.env(k, v);
    }
    c.args(args);
    run(c)
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn json_of(o: &Output) -> Value {
    let line = String::from_utf8_lossy(&o.stdout)
        .lines()
        .last()
        .unwrap_or("")
        .to_string();
    serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {}", text(o)))
}

fn code(o: &Output) -> i32 {
    o.status.code().unwrap_or(-1)
}

/// The routes a test allows, and the tracker it points at.
fn api_env<'a>(t: &'a FakeTracker, routes: &'a str) -> Vec<(&'a str, String)> {
    vec![
        ("CIGI_ROUTES", routes.to_string()),
        ("CIG_GITHUB_TOKEN", TOKEN.to_string()),
        ("CIGI_GITHUB_API", t.url()),
    ]
}

fn with<'a>(v: &'a [(&'a str, String)]) -> Vec<(&'a str, &'a str)> {
    v.iter().map(|(k, x)| (*k, x.as_str())).collect()
}

/// Hand a report in as another product would; the id and the fingerprint come back.
fn submit(fx: &Fixture, origin: &str, body: &Value) -> (String, String) {
    let f = fx.user_home.join(format!(
        "handed-in-{}.json",
        body["panic_message"].as_str().unwrap_or("x").len()
    ));
    std::fs::write(&f, body.to_string()).unwrap();
    let o = cigi(
        fx,
        &[],
        &[
            "--json",
            "crash",
            "submit",
            "--origin",
            origin,
            "--file",
            f.to_str().unwrap(),
        ],
    );
    assert_eq!(code(&o), 0, "{}", text(&o));
    let d = json_of(&o)["data"].clone();
    (
        d["id"].as_str().unwrap().to_string(),
        d["fingerprint"].as_str().unwrap().to_string(),
    )
}

fn plain_crash() -> Value {
    json!({
        "panic_message": "tin object hash mismatch", "panic_location": "E902",
        "version": "1.20.1", "kind": "crash",
        "backtrace": ["cig::tin::verify", "cig::burn::apply"],
    })
}

fn send_json(fx: &Fixture, env: &[(&str, String)], id: &str, extra: &[&str]) -> Output {
    let mut args = vec!["--json", "crash", "send", id];
    args.extend_from_slice(extra);
    cigi(fx, &with(env), &args)
}

/// The hive's reader, written again here on purpose: the last ```cig-report v1 fence of a body.
fn hive_parse(body: &str) -> Value {
    let lines: Vec<&str> = body.lines().collect();
    let start = lines
        .iter()
        .rposition(|l| *l == "```cig-report v1")
        .expect("a block");
    let end = lines[start + 1..]
        .iter()
        .position(|l| *l == "```")
        .expect("closed")
        + start
        + 1;
    serde_json::from_str(&lines[start + 1..end].join("\n")).expect("the block is JSON")
}

#[cfg(unix)]
fn fake_gh(dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir).unwrap();
    let p = dir.join("gh");
    std::fs::write(
        &p,
        r#"#!/bin/sh
echo "$*" >> "$GH_DIR/argv.log"
if [ "$1" = auth ]; then
  if [ -e "$GH_DIR/logged-out" ]; then echo "You are not logged in" >&2; exit 1; fi
  exit 0
fi
method="$3"; path="$4"
case "$method $path" in
  "GET search/issues") echo '{"total_count":0,"items":[]}';;
  "POST repos/"*"/comments") cat > "$GH_DIR/comment.json"; echo '{"id":1}';;
  "POST repos/"*"/issues") cat > "$GH_DIR/create.json"; echo '{"number":7,"html_url":"https://github.com/otmof-ops/cig-script/issues/7"}';;
  *) echo '{"message":"Not Found"}'; echo "gh: Not Found (HTTP 404)" >&2; exit 1;;
esac
"#,
    )
    .unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

fn queued(fx: &Fixture) -> Value {
    let o = cigi(fx, &[], &["--json", "crash", "status"]);
    assert_eq!(code(&o), 0, "{}", text(&o));
    json_of(&o)["data"].clone()
}

// ---- every route ---------------------------------------------------------------------------

#[test]
fn every_route_against_the_fake_tracker() {
    // the API route: the dry request is a search for issues only, then the report is filed
    let fx = Fixture::new();
    let t = FakeTracker::start();
    t.require_token(TOKEN);
    let env = api_env(&t, "api,link,file");
    let o = cigi(&fx, &with(&env), &["--json", "crash", "test-route"]);
    assert_eq!(code(&o), 0, "{}", text(&o));
    let d = json_of(&o)["data"].clone();
    let state = |r: &str| {
        d["routes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["route"] == r)
            .unwrap_or_else(|| panic!("a line for {r}: {d}"))["state"]
            .clone()
    };
    assert_eq!(state("api"), "ok");
    assert_eq!(state("link"), "ok");
    assert_eq!(state("file"), "ok");
    assert_eq!(state("gh"), "unavailable", "turned off by CIGI_ROUTES");
    assert_eq!(state("relay"), "blocked");
    assert_eq!(state("scoped-token"), "blocked");
    let reqs = t.requests();
    assert_eq!(reqs.len(), 1, "one dry request: {reqs:?}");
    assert_eq!(reqs[0].method, "GET");
    assert_eq!(reqs[0].path, "/search/issues");
    let q = reqs[0].q().unwrap();
    assert!(
        q.contains("is:issue") && q.contains("repo:otmof-ops/cig-installer"),
        "{q}"
    );
    assert!(t.issues().is_empty(), "a dry request writes nothing");

    // filing: created once, then a second report with the same fingerprint joins it
    let (id1, fp) = submit(&fx, "cig", &plain_crash());
    let o = send_json(&fx, &env, &id1, &[]);
    assert_eq!(code(&o), 0, "{}", text(&o));
    let issues = t.issues();
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].repo, CIG);
    assert!(
        issues[0].labels.contains(&"kind:crash".to_string()),
        "{:?}",
        issues[0].labels
    );
    assert!(issues[0].body.contains(&fp));
    let post = t
        .requests()
        .into_iter()
        .find(|r| r.method == "POST")
        .unwrap();
    assert_eq!(
        post.header("authorization"),
        Some(&*format!("Bearer {TOKEN}"))
    );
    assert_eq!(post.header("user-agent"), Some("ciginstaller"));
    assert_eq!(post.query, None);
    // the same report again is already filed: nothing is requested
    let n = t.requests().len();
    let o = send_json(&fx, &env, &id1, &[]);
    assert_eq!(code(&o), 0, "{}", text(&o));
    assert_eq!(t.requests().len(), n);

    // the link route: a prefilled page, never a request
    let n = t.requests().len();
    let (id2, _) = submit(
        &fx,
        "cig",
        &json!({"panic_message": "another fault", "panic_location": "E901", "version": "1.20.1"}),
    );
    let o = send_json(&fx, &env, &id2, &["--via", "link"]);
    assert_eq!(code(&o), 0, "{}", text(&o));
    let link = json_of(&o)["data"]["handed"][0]["where"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        link.starts_with("https://github.com/otmof-ops/cig-script/issues/new?title="),
        "{link}"
    );
    assert!(
        link.contains("labels=kind%3Acrash")
            || link.contains("labels=kind%3Ainternal")
            || link.contains("labels=kind"),
        "{link}"
    );
    assert!(link.len() <= 7_000);
    assert_eq!(t.requests().len(), n, "a link is never a request");

    // the file route: exported, owner-only, never overwritten
    let out = fx.user_home.join("export.json");
    let o = send_json(
        &fx,
        &env,
        &id2,
        &["--via", "file", "--out", out.to_str().unwrap()],
    );
    assert_eq!(code(&o), 0, "{}", text(&o));
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    assert_eq!(doc["repo"], CIG);
    assert!(doc["body"].as_str().unwrap().contains("```cig-report v1"));
    let o = send_json(
        &fx,
        &env,
        &id2,
        &["--via", "file", "--out", out.to_str().unwrap()],
    );
    assert_eq!(code(&o), 1, "an existing file is not overwritten");
    assert_eq!(t.requests().len(), n);
    assert_eq!(t.issues().len(), 1, "link and file file nothing");
}

#[test]
fn the_api_route_shows_githubs_error_body() {
    let fx = Fixture::new();
    let t = FakeTracker::start();
    t.require_token(TOKEN);
    let env = api_env(&t, "api");
    // 422 on the search: the cig-script#126 failure, shown not hidden
    t.fail(
        Endpoint::Search,
        Fault::Status(422, "Validation Failed".into()),
        1,
    );
    let o = cigi(&fx, &with(&env), &["--json", "crash", "test-route"]);
    assert_eq!(code(&o), 1, "{}", text(&o));
    let t1 = text(&o);
    assert!(
        t1.contains("E1857") && t1.contains("422") && t1.contains("Validation Failed"),
        "{t1}"
    );
    // 403
    t.fail(
        Endpoint::Search,
        Fault::Status(
            403,
            "Resource not accessible by personal access token".into(),
        ),
        1,
    );
    let o = cigi(&fx, &with(&env), &["--json", "crash", "test-route"]);
    let t2 = text(&o);
    assert_eq!(code(&o), 1, "{t2}");
    assert!(
        t2.contains("403") && t2.contains("Resource not accessible") && t2.contains("issues scope"),
        "{t2}"
    );
    // a rate limit says when it lifts
    t.fail(Endpoint::Search, Fault::RateLimit(429), 1);
    let o = cigi(&fx, &with(&env), &["--json", "crash", "test-route"]);
    let t3 = text(&o);
    assert_eq!(code(&o), 1, "{t3}");
    assert!(t3.contains("rate limited until 20"), "{t3}");
    // a bad token
    let mut bad = env.clone();
    bad[1].1 = "wrong".into();
    let o = cigi(&fx, &with(&bad), &["--json", "crash", "test-route"]);
    let t4 = text(&o);
    assert_eq!(code(&o), 1, "{t4}");
    assert!(
        t4.contains("401") && t4.contains("Bad credentials") && !t4.contains("wrong"),
        "{t4}"
    );
    // and the token never appears in anything printed
    for s in [&t1, &t2, &t3, &t4] {
        assert!(!s.contains(TOKEN));
    }
    // the doctor's data: each failure was kept
    let st = queued(&fx);
    let api = st["routes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["route"] == "api")
        .unwrap()
        .clone();
    assert!(
        api["last_failure"].is_string() && api["last_error"].is_string(),
        "{api}"
    );
}

#[cfg(unix)]
#[test]
fn the_gh_route_goes_through_the_persons_login() {
    let fx = Fixture::new();
    let dir = fx.user_home.join("ghbin");
    fake_gh(&dir);
    let gh_dir = fx.user_home.join("ghlog");
    std::fs::create_dir_all(&gh_dir).unwrap();
    let path = format!("{}:/usr/bin:/bin", dir.display());
    let env = [
        ("CIGI_ROUTES", "gh"),
        ("PATH", path.as_str()),
        ("GH_DIR", gh_dir.to_str().unwrap()),
    ];
    let o = cigi(&fx, &env, &["--json", "crash", "test-route"]);
    assert_eq!(code(&o), 0, "{}", text(&o));
    assert!(json_of(&o)["data"]["can_file"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r == "gh"));
    let argv = std::fs::read_to_string(gh_dir.join("argv.log")).unwrap();
    assert!(
        argv.contains("api -X GET search/issues") && argv.contains("is:issue"),
        "{argv}"
    );
    let (id, _) = submit(&fx, "cig", &plain_crash());
    let o = cigi(&fx, &env, &["--json", "crash", "send", &id]);
    assert_eq!(code(&o), 0, "{}", text(&o));
    assert_eq!(
        json_of(&o)["data"]["sent"][0]["url"],
        "https://github.com/otmof-ops/cig-script/issues/7"
    );
    let created: Value =
        serde_json::from_str(&std::fs::read_to_string(gh_dir.join("create.json")).unwrap())
            .unwrap();
    let mut keys: Vec<&str> = created
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, ["body", "labels", "title"]);
    assert_eq!(
        hive_parse(created["body"].as_str().unwrap())["blame"]["repo"],
        CIG
    );
}

// ---- blame ---------------------------------------------------------------------------------

#[test]
fn blame_routes_to_the_right_repo() {
    let fx = Fixture::new();
    let t = FakeTracker::start();
    t.require_token(TOKEN);
    let env = api_env(&t, "api");
    // a cig E900 seen under cigo: the top frame is in cig, so it is cig's bug
    let (id, fp) = submit(
        &fx,
        "cig-orchestrator",
        &json!({
            "panic_message": "internal error in a builtin", "panic_location": "E900",
            "version": "1.17.0", "kind": "internal",
            "backtrace": ["cig::builtins::raise", "cigo::run::orchestrate"],
        }),
    );
    // cigo has an issue for it already
    let theirs = t.seed_issue(CIGO, "E900 under cigo", &format!("fingerprint {fp}"));
    let o = send_json(&fx, &env, &id, &[]);
    assert_eq!(code(&o), 0, "{}", text(&o));
    let issues = t.issues();
    let filed: Vec<_> = issues.iter().filter(|i| i.repo == CIG).collect();
    assert_eq!(filed.len(), 1, "filed in cig-script: {issues:#?}");
    assert!(
        filed[0].labels.contains(&"area:cig".to_string()),
        "{:?}",
        filed[0].labels
    );
    let block = hive_parse(&filed[0].body);
    assert_eq!(
        block["blame"],
        json!({"product": "cig", "repo": CIG, "via": "frames"})
    );
    assert_eq!(block["origin"], "cig-orchestrator");
    // and a line in cigo's issue says where it went
    let origin = issues
        .iter()
        .find(|i| i.repo == CIGO && i.number == theirs)
        .unwrap();
    assert_eq!(origin.comments.len(), 1, "{origin:#?}");
    assert!(
        origin.comments[0].contains(&format!("/{CIG}/issues/")),
        "{}",
        origin.comments[0]
    );
    assert_eq!(
        issues.iter().filter(|i| i.repo == CIGO).count(),
        1,
        "nothing new in cigo"
    );

    // no frames: the code's own range decides (E1010 is CigCompile's)
    let (id2, _) = submit(
        &fx,
        "cig-orchestrator",
        &json!({"panic_message": "a program would not build", "panic_location": "E1010", "version": "1.17.0"}),
    );
    let o = send_json(&fx, &env, &id2, &[]);
    assert_eq!(code(&o), 0, "{}", text(&o));
    let c = t
        .issues()
        .into_iter()
        .find(|i| i.repo == "otmof-ops/cig-compile")
        .expect("filed in cig-compile");
    assert_eq!(hive_parse(&c.body)["blame"]["via"], "code");

    // nothing to go on: where it was seen
    let (id3, _) = submit(
        &fx,
        "cig-compile",
        &json!({"panic_message": "plain panic", "version": "0.2.0"}),
    );
    let o = send_json(&fx, &env, &id3, &[]);
    assert_eq!(code(&o), 0, "{}", text(&o));
    assert!(t
        .issues()
        .iter()
        .any(|i| i.repo == "otmof-ops/cig-compile" && i.title.contains("plain panic")));

    // a moved issue is followed: the maintainers transfer it, the report joins it where it is
    let (id4, fp4) = submit(
        &fx,
        "cig",
        &json!({"panic_message": "moved fault", "panic_location": "E903", "version": "1.20.1"}),
    );
    let old = t.seed_issue(CIG, "moved fault", &format!("fingerprint {fp4}"));
    let new = t.transfer(CIG, old, "otmof-ops/cig-installer");
    let o = send_json(&fx, &env, &id4, &[]);
    assert_eq!(code(&o), 0, "{}", text(&o));
    let moved = t
        .issues()
        .into_iter()
        .find(|i| i.repo == "otmof-ops/cig-installer" && i.number == new)
        .unwrap();
    assert_eq!(
        moved.comments.len(),
        1,
        "the sighting landed on the issue where it lives now"
    );
    assert!(json_of(&o)["data"]["sent"][0]["url"]
        .as_str()
        .unwrap()
        .contains("cig-installer/issues/"));
    assert!(!t
        .issues()
        .iter()
        .any(|i| i.title == "moved fault" && i.repo == CIG));
}

// ---- the machine block ---------------------------------------------------------------------

#[test]
fn machine_block_round_trips() {
    let fx = Fixture::new();
    let t = FakeTracker::start();
    t.require_token(TOKEN);
    let env = api_env(&t, "api");
    let repro = json!({"script": "say \"hi\"\n```\nthen `more`", "verified": true, "verification": "ran in a sandbox, same fault"});
    let (id, fp) = submit(
        &fx,
        "cig",
        &json!({
            "panic_message": "rollback tripwire: an undone burn left a file", "panic_location": "T-rollback",
            "version": "1.20.1", "kind": "tripwire", "os": "linux", "arch": "x86_64",
            "backtrace": ["cig::burn::unburn"],
            "repro": repro,
            "carton": "0.2.0", "version_matrix": {"cig": "1.20.1", "cigo": "1.17.0"},
            "first_bad_carton": "0.1.9",
        }),
    );
    let o = send_json(&fx, &env, &id, &[]);
    assert_eq!(code(&o), 0, "{}", text(&o));
    let issue = t.issues().remove(0);
    let b = hive_parse(&issue.body);
    assert_eq!(b["schema"], "cig-report");
    assert_eq!(b["version"], 1);
    assert_eq!(b["fingerprint"], fp.as_str());
    assert_eq!(b["id"], id.as_str());
    assert_eq!(b["kind"], "tripwire");
    assert_eq!(b["origin"], "cig");
    assert_eq!(b["origin_version"], "1.20.1");
    assert_eq!(
        b["blame"],
        json!({"product": "cig", "repo": CIG, "via": "frames"})
    );
    assert_eq!(b["matrix"], json!({"cig": "1.20.1", "cigo": "1.17.0"}));
    assert_eq!(b["carton"], "0.2.0");
    assert_eq!(b["first_bad_carton"], "0.1.9");
    assert_eq!(b["regression"], true);
    assert_eq!(b["repro"]["verified"], true);
    assert_eq!(
        b["repro"]["script"], repro["script"],
        "a repro with a fence in it survives"
    );
    // the labels the pipeline set are the block's labels: a tripwire on rollback is high
    let labels: Vec<String> = b["labels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l.as_str().unwrap().to_string())
        .collect();
    for want in ["kind:tripwire", "area:cig", "severity:high", "regression"] {
        assert!(labels.iter().any(|l| l == want), "{want} in {labels:?}");
        assert!(
            issue.labels.iter().any(|l| l == want),
            "{want} on the issue {:?}",
            issue.labels
        );
    }
    // a fence in the text never ends the block early
    assert_eq!(issue.body.matches("```cig-report v1").count(), 1);
    assert!(
        issue.body.trim_end().ends_with("```"),
        "the block is at the end"
    );
}

// ---- fault injection -----------------------------------------------------------------------

fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

fn one_issue(t: &FakeTracker) {
    assert_eq!(t.issues().len(), 1, "{:#?}", t.issues());
}

#[test]
fn fault_injection_files_exactly_once() {
    // network down: queued with the reason, filed once when it is back
    let fx = Fixture::new();
    let (id, _) = submit(&fx, "cig", &plain_crash());
    let dead = format!("http://127.0.0.1:{}", free_port());
    let env = [
        ("CIGI_ROUTES", "api"),
        ("CIG_GITHUB_TOKEN", TOKEN),
        ("CIGI_GITHUB_API", dead.as_str()),
    ];
    let o = cigi(&fx, &env, &["--json", "crash", "send", &id]);
    assert_eq!(code(&o), 1, "{}", text(&o));
    assert!(
        text(&o).contains("E1857") && text(&o).contains("could not connect"),
        "{}",
        text(&o)
    );
    let q = queued(&fx);
    assert_eq!(q["queued"], 1);
    assert!(q["queue"][0]["last_error"]
        .as_str()
        .unwrap()
        .contains("could not connect"));
    let t = FakeTracker::start();
    t.require_token(TOKEN);
    let up = api_env(&t, "api");
    let o = send_json(&fx, &up, &id, &[]);
    assert_eq!(code(&o), 0, "{}", text(&o));
    one_issue(&t);
    assert_eq!(queued(&fx)["queued"], 0);

    // DNS failing: a name that cannot resolve
    let fx = Fixture::new();
    let (id, _) = submit(&fx, "cig", &plain_crash());
    let env = [
        ("CIGI_ROUTES", "api"),
        ("CIG_GITHUB_TOKEN", TOKEN),
        ("CIGI_GITHUB_API", "https://no-tracker.invalid"),
    ];
    let o = cigi(&fx, &env, &["--json", "crash", "send", &id]);
    assert_eq!(code(&o), 1, "{}", text(&o));
    assert!(
        text(&o).contains("no-tracker.invalid") && text(&o).contains("E1857"),
        "{}",
        text(&o)
    );
    assert_eq!(queued(&fx)["queued"], 1);

    // gh missing: nothing can file, the report waits, a link is offered
    let fx = Fixture::new();
    let (id, _) = submit(&fx, "cig", &plain_crash());
    let empty = fx.user_home.join("empty-path");
    std::fs::create_dir_all(&empty).unwrap();
    let env = [
        ("CIGI_ROUTES", "gh,link"),
        ("PATH", empty.to_str().unwrap()),
    ];
    let o = cigi(&fx, &env, &["--json", "crash", "send", &id]);
    assert_eq!(code(&o), 0, "{}", text(&o));
    assert!(text(&o).contains("gh was not found"), "{}", text(&o));
    assert!(json_of(&o)["data"]["link"]
        .as_str()
        .unwrap()
        .starts_with("https://github.com/"));
    assert_eq!(queued(&fx)["queued"], 1);

    #[cfg(unix)]
    {
        // gh logged out: it is passed over, the token's route files, once
        let fx = Fixture::new();
        let dir = fx.user_home.join("ghbin");
        fake_gh(&dir);
        let gh_dir = fx.user_home.join("ghlog");
        std::fs::create_dir_all(&gh_dir).unwrap();
        std::fs::write(gh_dir.join("logged-out"), "").unwrap();
        let (id, _) = submit(&fx, "cig", &plain_crash());
        let t = FakeTracker::start();
        t.require_token(TOKEN);
        let path = format!("{}:/usr/bin:/bin", dir.display());
        let mut env = api_env(&t, "gh,api");
        env.push(("PATH", path));
        env.push(("GH_DIR", gh_dir.display().to_string()));
        let o = send_json(&fx, &env, &id, &[]);
        assert_eq!(code(&o), 0, "{}", text(&o));
        one_issue(&t);
        assert!(
            !gh_dir.join("create.json").exists(),
            "the logged-out gh filed nothing"
        );
    }

    // a token without the scope: GitHub's reason shown, queued, then filed once
    let fx = Fixture::new();
    let t = FakeTracker::start();
    t.require_token(TOKEN);
    let env = api_env(&t, "api");
    let (id, _) = submit(&fx, "cig", &plain_crash());
    t.fail(
        Endpoint::Create,
        Fault::Status(
            403,
            "Resource not accessible by personal access token".into(),
        ),
        1,
    );
    let o = send_json(&fx, &env, &id, &[]);
    assert_eq!(code(&o), 1, "{}", text(&o));
    assert!(
        text(&o).contains("Resource not accessible") && text(&o).contains("issues scope"),
        "{}",
        text(&o)
    );
    assert!(t.issues().is_empty());
    let o = send_json(&fx, &env, &id, &[]);
    assert_eq!(code(&o), 0, "{}", text(&o));
    one_issue(&t);

    // a 422 on the search: no blind create, queued with GitHub's words
    let fx = Fixture::new();
    let t = FakeTracker::start();
    t.require_token(TOKEN);
    let env = api_env(&t, "api");
    let (id, _) = submit(&fx, "cig", &plain_crash());
    t.fail(
        Endpoint::Search,
        Fault::Status(422, "Validation Failed".into()),
        1,
    );
    let o = send_json(&fx, &env, &id, &[]);
    assert_eq!(code(&o), 1, "{}", text(&o));
    assert!(
        text(&o).contains("422") && text(&o).contains("Validation Failed"),
        "{}",
        text(&o)
    );
    assert!(
        t.requests().iter().all(|r| r.method == "GET"),
        "nothing was written"
    );
    assert!(t.issues().is_empty());
    let o = send_json(&fx, &env, &id, &[]);
    assert_eq!(code(&o), 0, "{}", text(&o));
    one_issue(&t);

    // a rate limit: the next try is recorded
    let fx = Fixture::new();
    let t = FakeTracker::start();
    t.require_token(TOKEN);
    let env = api_env(&t, "api");
    let (id, _) = submit(&fx, "cig", &plain_crash());
    t.fail(Endpoint::Search, Fault::RateLimit(403), 1);
    let o = send_json(&fx, &env, &id, &[]);
    assert_eq!(code(&o), 1, "{}", text(&o));
    assert!(queued(&fx)["queue"][0]["next_try"].is_string());
    let o = send_json(&fx, &env, &id, &[]);
    assert_eq!(code(&o), 0, "{}", text(&o));
    one_issue(&t);

    // the answer is lost after the write: the retry knows the issue is its own
    let fx = Fixture::new();
    let t = FakeTracker::start();
    t.require_token(TOKEN);
    let env = api_env(&t, "api");
    let (id, _) = submit(&fx, "cig", &plain_crash());
    t.fail(Endpoint::Create, Fault::AnswerAfterWrite(502), 1);
    let o = send_json(&fx, &env, &id, &[]);
    assert_eq!(code(&o), 1, "{}", text(&o));
    one_issue(&t);
    let o = send_json(&fx, &env, &id, &[]);
    assert_eq!(code(&o), 0, "{}", text(&o));
    one_issue(&t);
    assert!(
        t.issues()[0].comments.is_empty(),
        "no second write for the same report"
    );
    assert!(
        text(&o).contains("already filed by this report"),
        "{}",
        text(&o)
    );
}

// ---- consent and the allowlist (#19) -------------------------------------------------------

#[test]
fn no_request_leaves_without_consent_and_nothing_outside_the_allowlist_travels() {
    let fx = Fixture::new();
    let t = FakeTracker::start();
    t.require_token(TOKEN);
    let env = api_env(&t, "api,link,file");
    // a report with things that must stay home
    let secret_path = "/home/tessa-q/projects/bunnyhutch-secret/plan.cig";
    let (id, _) = submit(
        &fx,
        "cig",
        &json!({
            "panic_message": format!("cannot read {secret_path} on bunnyhutch"),
            "panic_location": "E901", "version": "1.20.1",
            "command": ["cig", "run", secret_path],
            "env": {"AWS_SECRET_ACCESS_KEY": "hunter2-secret-value"},
            "secrets": ["hunter2-secret-value"],
            "backtrace": ["cig::run::exec"],
        }),
    );
    // never, anywhere: nothing is requested by any command
    let mut never = with(&env);
    never.push(("CIG_REPORTS", "never"));
    for args in [
        vec!["--json", "crash", "send", id.as_str()],
        vec!["--json", "crash", "send", "--all"],
        vec!["--json", "crash", "test-route"],
        vec!["--json", "crash", "status"],
        vec!["--json", "crash", "list"],
        vec!["--json", "crash", "show", id.as_str()],
    ] {
        let o = cigi(&fx, &never, &args);
        let t_ = text(&o);
        if args[2] == "send" {
            assert_eq!(code(&o), 2, "{args:?}: {t_}");
        }
        assert!(!t_.contains(TOKEN));
    }
    assert!(
        t.requests().is_empty(),
        "reports = never: {:?}",
        t.requests()
    );
    // saving a report asks nobody and sends nothing: only a typed send is the yes
    let (_id2, _) = submit(
        &fx,
        "cig",
        &json!({"panic_message": "second", "version": "1"}),
    );
    assert!(t.requests().is_empty());
    // the yes: what leaves is the allowlisted view and nothing else
    let o = send_json(&fx, &env, &id, &[]);
    assert_eq!(code(&o), 0, "{}", text(&o));
    let reqs = t.requests();
    assert!(!reqs.is_empty());
    for r in &reqs {
        assert_eq!(r.header("user-agent"), Some("ciginstaller"));
        assert!(r.header("cookie").is_none(), "{r:?}");
        for (k, _) in &r.headers {
            let k = k.to_ascii_lowercase();
            assert!(
                [
                    "host",
                    "user-agent",
                    "accept",
                    "accept-encoding",
                    "connection",
                    "content-type",
                    "content-length",
                    "authorization",
                    "x-github-api-version"
                ]
                .contains(&k.as_str()),
                "header {k} is outside the set"
            );
        }
        if r.method == "GET" {
            let qs = r.query.clone().unwrap_or_default();
            let keys: Vec<&str> = qs.split('&').filter_map(|p| p.split('=').next()).collect();
            assert!(keys.iter().all(|k| ["q", "per_page"].contains(k)), "{qs}");
            let q = r.q().unwrap();
            assert!(
                q.split_whitespace().all(|w| w.starts_with("repo:")
                    || w == "is:issue"
                    || w == "in:body"
                    || w.len() == 8),
                "{q}"
            );
        } else {
            assert_eq!(r.query, None);
            let b: Value = serde_json::from_str(&r.body).unwrap();
            let mut keys: Vec<&str> = b.as_object().unwrap().keys().map(String::as_str).collect();
            keys.sort_unstable();
            assert!(
                keys == ["body", "labels", "title"] || keys == ["body"],
                "{keys:?}"
            );
        }
        for secret in [
            "tessa-q",
            "bunnyhutch",
            "hunter2",
            "AWS_SECRET",
            secret_path,
            TOKEN,
        ] {
            // the token is a header, never a body
            if r.header("authorization").is_some() && secret == TOKEN {
                continue;
            }
            assert!(!r.body.contains(secret), "{secret} travelled: {}", r.body);
            assert!(!r.query.clone().unwrap_or_default().contains(secret));
        }
    }
    // the block holds only what the allowlist and the derivation hold
    let b = hive_parse(&t.issues()[0].body);
    let known = [
        "schema",
        "version",
        "id",
        "fingerprint",
        "kind",
        "origin",
        "origin_version",
        "os",
        "arch",
        "blame",
        "matrix",
        "carton",
        "repro",
        "first_bad_carton",
        "regression",
        "labels",
    ];
    for k in b.as_object().unwrap().keys() {
        assert!(known.contains(&k.as_str()), "{k} is not a block field");
    }
}

// ---- status and the doctor -----------------------------------------------------------------

#[test]
fn status_and_the_doctor_show_the_queue_the_routes_and_the_consent() {
    let mut fx = common::fixture();
    fx.carton("0.1.0", &[("cig", "1.20.0")]);
    fx.publish("stable");
    let o = common::cigi(&fx, &["install", "carton", "--carton", "0.1.0"]);
    common::ok(&o);
    let t = FakeTracker::start();
    t.require_token(TOKEN);
    let (id, _) = submit(&fx, "cig", &plain_crash());
    let env = api_env(&t, "api");
    // a failed attempt, then the doctor shows all three
    t.fail(
        Endpoint::Search,
        Fault::Status(422, "Validation Failed".into()),
        1,
    );
    let o = send_json(&fx, &env, &id, &[]);
    assert_eq!(code(&o), 1);
    let q = queued(&fx);
    assert_eq!(q["queued"], 1);
    assert_eq!(q["queue"][0]["id"], id.as_str());
    assert_eq!(q["queue"][0]["target"], CIG);
    assert_eq!(q["reports_setting"], "ask");
    assert_eq!(q["blocked"].as_array().unwrap().len(), 3);
    let mut c = fx.cigi();
    c.env("CIGI_ALLOW_SINGLE_SOURCE", "true")
        .args(["--yes", "--json", "doctor"]);
    let d = json_of(&run(c))["data"].clone();
    let reports: Vec<&Value> = d["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["section"] == "reports")
        .collect();
    assert!(
        reports.iter().any(|f| f["found"]
            .as_str()
            .unwrap()
            .contains("1 report(s) queued, 1 with a failed attempt; reports = ask")),
        "{reports:#?}"
    );
    assert!(
        reports.iter().any(|f| f["found"]
            .as_str()
            .unwrap()
            .contains("route api: last success never, last failure 20")),
        "{reports:#?}"
    );
    // after a success the route has both
    let o = send_json(&fx, &env, &id, &[]);
    assert_eq!(code(&o), 0, "{}", text(&o));
    let st = queued(&fx);
    let api = st["routes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["route"] == "api")
        .unwrap()
        .clone();
    assert!(
        api["last_success"].is_string() && api["last_failure"].is_string(),
        "{api}"
    );
    assert_eq!(st["queued"], 0);
}
