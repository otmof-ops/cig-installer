// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! The machine layer and locked settings (#17): what an organization puts on a machine, and
//! that a person cannot move it.
//!
//! The binary reads the layer from `<scratch>/machine/config.toml` (the `test-machine-root`
//! feature, which only test builds have; the fixture points every child there, so no test reads
//! the real `/etc/cigscript`). The macOS and Windows readers run through the same trait in
//! `cigi-home`'s unit tests, with fixtures. The products are the fixture's fakes, so these tests
//! are Unix only.
#![cfg(unix)]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Output;
use std::sync::{Arc, Mutex};

use cigi_testkit::Fixture;
use common::*;
use serde_json::Value;

const FIVE: [(&str, &str, &str, &str); 5] = [
    // key, the locked value, the person's file, the environment
    ("carton", "0.1.0", "0.1.1", "0.1.1"),
    ("auto_update", "off", "all", "security"),
    ("reports", "never", "always", "ask"),
    (
        "mirror",
        "https://org.example.com/m",
        "file:///mine",
        "file:///env",
    ),
    ("preset", "strict", "chill", "balanced"),
];

const LOCKED_ALL: &str =
    "locked = [\"carton\", \"auto_update\", \"reports\", \"mirror\", \"preset\"]\n";

fn layer_path(fx: &Fixture) -> PathBuf {
    fx.machine_root().join("config.toml")
}

/// The machine layer as an administrator leaves it: readable, writable by nobody else.
fn layer(fx: &Fixture, text: &str) {
    std::fs::create_dir_all(fx.machine_root()).unwrap();
    let p = layer_path(fx);
    std::fs::write(&p, text).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
}

fn all_five_locked(fx: &Fixture) {
    let mut t = String::new();
    for (k, v, ..) in FIVE {
        t.push_str(&format!("{k} = \"{v}\"\n"));
    }
    t.push_str(LOCKED_ALL);
    layer(fx, &t);
}

/// The person's own `installer/config`.
fn persons_file(fx: &Fixture, text: &str) {
    let p = fx.home.config_path();
    // the folders with the modes the installer insists on (this machine's umask may be looser)
    for d in [fx.cigscript(), p.parent().unwrap().to_path_buf()] {
        std::fs::create_dir_all(&d).unwrap();
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::fs::write(p, text).unwrap();
}

fn persons_text(fx: &Fixture) -> String {
    std::fs::read_to_string(fx.home.config_path()).unwrap_or_default()
}

fn run(fx: &Fixture, env: &[(&str, &str)], args: &[&str]) -> Output {
    let mut c = fx.cigi();
    c.env("CIGI_ALLOW_SINGLE_SOURCE", "true")
        .env("USER", "tessa-q")
        .env("HOSTNAME", "bunnyhutch");
    for (k, v) in env {
        c.env(k, v);
    }
    c.args(args);
    cigi_testkit::output_retrying(&mut c).expect("run ciginstaller")
}

fn data(o: &Output) -> Value {
    let line = String::from_utf8_lossy(&o.stdout)
        .lines()
        .last()
        .unwrap_or("")
        .to_string();
    let v: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {}", text(o)));
    v["data"].clone()
}

fn var(key: &str) -> String {
    format!("CIGI_{}", key.to_ascii_uppercase())
}

#[test]
fn a_locked_value_beats_the_environment_and_the_persons_file_for_each_of_the_five_keys() {
    let fx = fixture();
    all_five_locked(&fx);
    let mut file = String::new();
    for (k, _, f, _) in FIVE {
        file.push_str(&format!("{k} = {f}\n"));
    }
    persons_file(&fx, &file);
    let store = layer_path(&fx).display().to_string();
    for (k, locked, _, env) in FIVE {
        let v = var(k);
        let o = run(&fx, &[(v.as_str(), env)], &["--json", "config", "get", k]);
        assert_eq!(o.status.code(), Some(0), "{k}: {}", text(&o));
        let d = data(&o);
        assert_eq!(d["value"], locked, "{k}: {d}");
        assert_eq!(d["source"], "locked", "{k}");
        assert_eq!(d["locked"], true, "{k}");
        assert_eq!(d["locked_by"], store.as_str(), "{k}");
    }
    // the same through `list`, with all of them at once
    let envs: Vec<(String, &str)> = FIVE.iter().map(|(k, _, _, e)| (var(k), *e)).collect();
    let pairs: Vec<(&str, &str)> = envs.iter().map(|(k, e)| (k.as_str(), *e)).collect();
    let d = data(&run(&fx, &pairs, &["--json", "config", "list"]));
    for (k, locked, ..) in FIVE {
        assert_eq!(d["config"][k], locked, "{k}: {d}");
    }
}

#[test]
fn a_flag_does_not_beat_a_lock_either() {
    // `--preset` and `--reports` are the flags of these five keys; the others have none, and
    // the unit tests in cigi-home push `set_for_run` at every one
    let fx = fixture();
    all_five_locked(&fx);
    for (flag, value, key) in [
        ("--preset", "chill", "preset"),
        ("--reports", "always", "reports"),
    ] {
        let o = run(&fx, &[], &["install", "--dry-run", "--json", flag, value]);
        let t = text(&o);
        assert_eq!(o.status.code(), Some(2), "{flag}: {t}");
        assert!(
            t.contains("E1904") && t.contains(key) && t.contains("config.toml"),
            "{flag}: {t}"
        );
    }
    assert_eq!(persons_text(&fx), "", "nothing was written");
}

#[test]
fn set_and_unset_on_a_locked_key_refuse_with_the_code_and_the_source() {
    let fx = fixture();
    all_five_locked(&fx);
    persons_file(&fx, "keep_days = 30\n");
    let before = persons_text(&fx);
    let store = layer_path(&fx).display().to_string();
    for (k, _, f, _) in FIVE {
        for args in [vec!["config", "set", k, f], vec!["config", "unset", k]] {
            let o = run(&fx, &[], &args);
            let t = text(&o);
            assert_eq!(o.status.code(), Some(2), "{args:?}: {t}");
            assert!(t.contains("E1904"), "{args:?}: {t}");
            assert!(t.contains(&store), "the source is named: {t}");
            assert!(t.contains(k), "{t}");
        }
    }
    assert_eq!(persons_text(&fx), before, "a refusal writes nothing");
    // a key the layer does not lock still takes the person's word
    let o = run(&fx, &[], &["config", "set", "keep_days", "10"]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    assert!(persons_text(&fx).contains("keep_days = 10"));
}

#[test]
fn an_unlocked_machine_value_sits_below_the_persons_file_and_above_the_default() {
    let fx = fixture();
    layer(&fx, "keep_days = 45\nreports = \"ask\"\n");
    let store = layer_path(&fx).display().to_string();
    let get = |fx: &Fixture, k: &str| data(&run(fx, &[], &["--json", "config", "get", k]));
    let d = get(&fx, "keep_days");
    assert_eq!(
        (d["value"].as_str(), d["source"].as_str()),
        (Some("45"), Some("machine")),
        "{d}"
    );
    assert_eq!(d["locked"], false);
    let o = run(&fx, &[], &["config", "get", "keep_days"]);
    assert!(
        text(&o).contains("machine layer") && text(&o).contains(&store),
        "{}",
        text(&o)
    );
    ok(&run(&fx, &[], &["config", "set", "keep_days", "10"]));
    let d = get(&fx, "keep_days");
    assert_eq!(
        (d["value"].as_str(), d["source"].as_str()),
        (Some("10"), Some("file")),
        "{d}"
    );
    ok(&run(&fx, &[], &["config", "unset", "keep_days"]));
    assert_eq!(get(&fx, "keep_days")["value"], "45");
    // a setting the layer does not mention is the built-in default
    assert_eq!(get(&fx, "theme")["source"], "default");
}

#[test]
fn get_list_and_status_say_locked_by_the_store() {
    let fx = fixture();
    all_five_locked(&fx);
    let store = layer_path(&fx).display().to_string();
    let want = format!("locked by {store}");
    let t = text(&run(&fx, &[], &["config", "get", "carton"]));
    assert!(t.contains(&want), "{t}");
    let t = text(&run(&fx, &[], &["config"]));
    assert!(t.contains(&want), "{t}");
    for (k, ..) in FIVE {
        assert!(t.contains(k), "{k}: {t}");
    }
    // the inventory
    let o = run(&fx, &[], &["--json", "status"]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    let d = data(&o);
    assert_eq!(d["locks"]["source"], store.as_str(), "{d}");
    let keys: Vec<&str> = d["locks"]["locked"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["key"].as_str().unwrap())
        .collect();
    assert_eq!(
        keys,
        ["auto_update", "carton", "mirror", "preset", "reports"]
    );
    let reports = d["locks"]["locked"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["key"] == "reports")
        .unwrap();
    assert_eq!(reports["value"], "never");
    assert_eq!(reports["locked_by"], store.as_str());
    // a machine with no layer says so
    let bare = fixture();
    let d = data(&run(&bare, &[], &["--json", "status"]));
    assert!(
        d["locks"]["source"].is_null() && d["locks"]["locked"].as_array().unwrap().is_empty(),
        "{d}"
    );
}

#[test]
fn a_malformed_or_loose_machine_layer_is_an_error_that_names_it() {
    let fx = fixture();
    layer(&fx, "auto_update = \"sometimes\"\n");
    let store = layer_path(&fx).display().to_string();
    for args in [&["config"][..], &["status"], &["config", "get", "channel"]] {
        let o = run(&fx, &[], args);
        let t = text(&o);
        assert_ne!(o.status.code(), Some(0), "{args:?}: {t}");
        assert!(
            t.contains("E1830") && t.contains(&store) && t.contains("auto_update"),
            "{args:?}: {t}"
        );
    }
    layer(&fx, "locked = [\"nope\"]\n");
    assert!(text(&run(&fx, &[], &["config"])).contains("not a setting"));
    // anyone-can-write is not a lock anyone should trust
    layer(&fx, "reports = \"never\"\nlocked = [\"reports\"]\n");
    std::fs::set_permissions(layer_path(&fx), std::fs::Permissions::from_mode(0o666)).unwrap();
    let t = text(&run(&fx, &[], &["config"]));
    assert!(t.contains("E1830") && t.contains("write"), "{t}");
}

// ---- reports = never, locked -----------------------------------------------------------------

struct Route {
    base: String,
    seen: Arc<Mutex<Vec<String>>>,
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
                let _ = req.as_reader().read_to_string(&mut body);
                log.lock()
                    .unwrap()
                    .push(format!("{} {}", req.method(), req.url()));
                let r = tiny_http::Response::from_string(
                    r#"{"html_url":"https://github.com/otmof-ops/cig-installer/issues/41"}"#,
                );
                let _ = req.respond(r);
            }
        });
        Route {
            base: format!("http://127.0.0.1:{port}/reports"),
            seen,
        }
    }
    fn requests(&self) -> usize {
        self.seen.lock().unwrap().len()
    }
}

#[test]
fn reports_locked_to_never_stops_every_send_path_whatever_the_environment_says() {
    let fx = fixture();
    layer(&fx, "reports = \"never\"\nlocked = [\"reports\"]\n");
    let store = layer_path(&fx).display().to_string();
    let route = Route::start();
    // every way a person can ask for yes
    let yes = [
        ("CIG_REPORTS", "always"),
        ("CIGI_REPORTS", "always"),
        ("CIGI_REPORT_ROUTE", route.base.as_str()),
    ];
    // a report to send: another product hands one in
    let f = fx.user_home.join("handed-in.json");
    std::fs::write(&f, r#"{"panic_message":"boom","version":"1.20.0"}"#).unwrap();
    let o = run(
        &fx,
        &yes,
        &[
            "--json",
            "crash",
            "submit",
            "--origin",
            "cig",
            "--file",
            f.to_str().unwrap(),
        ],
    );
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    let id = data(&o)["id"].as_str().unwrap().to_string();
    persons_file(&fx, "reports = always\n");

    let out_file = fx
        .user_home
        .join("never-written.json")
        .display()
        .to_string();
    let paths: Vec<Vec<&str>> = vec![
        vec!["crash", "send", &id],
        vec!["crash", "send", "--all"],
        vec![
            "crash",
            "send",
            &id,
            "--via",
            "file",
            "--out",
            "/tmp/never-written.json",
        ],
        vec!["bug", "--did", "x", "--happened", "ok", "--send"],
        vec!["report", "--send"],
    ];
    for args in paths {
        let o = run(&fx, &yes, &args);
        let t = text(&o);
        assert_eq!(o.status.code(), Some(2), "{args:?}: {t}");
        assert!(
            t.contains("E1857") && t.contains("locked") && t.contains(&store),
            "{args:?}: {t}"
        );
    }
    assert_eq!(route.requests(), 0, "nothing may leave");
    assert!(!std::path::Path::new(&out_file).exists());
    // the setting reads back as the lock
    let d = data(&run(&fx, &yes, &["--json", "crash", "list"]));
    assert_eq!(d["reports_setting"], "never", "{d}");
    // and the person cannot unlock it from their file
    let o = run(&fx, &[], &["config", "set", "reports", "ask"]);
    assert!(text(&o).contains("E1904"), "{}", text(&o));
}

// ---- carton ------------------------------------------------------------------------------------

fn two_cartons() -> Fixture {
    let mut fx = fixture();
    fx.product("cig", "1.20.0").dry_run_output("plan: 1 op");
    fx.product("cig", "1.21.0").dry_run_output("plan: 1 op");
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.17.0")]);
    fx.carton("0.1.1", &[("cig", "1.21.0"), ("cigo", "1.17.0")]);
    fx.publish("stable");
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.1"]));
    fx
}

fn which(fx: &Fixture, env: &[(&str, &str)]) -> String {
    let o = run(fx, env, &["which", "cig"]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    text(&o)
}

#[test]
fn a_locked_carton_beats_a_projects_pin_the_environment_and_the_persons_file() {
    let fx = two_cartons();
    std::fs::write(fx.project.join(".cig-carton"), "0.1.1\n").unwrap();
    // without a lock the project's pin runs 0.1.1 (the baseline)
    assert!(which(&fx, &[]).contains("cig 1.21.0, from carton 0.1.1 (pinned in .cig-carton)"));
    layer(&fx, "carton = \"0.1.0\"\nlocked = [\"carton\"]\n");
    let store = layer_path(&fx).display().to_string();
    persons_file(&fx, "carton = 0.1.1\n");
    let t = which(&fx, &[("CIGI_CARTON", "0.1.1")]);
    assert!(
        t.contains("cig 1.20.0, from carton 0.1.0") && t.contains(&format!("locked by {store}")),
        "{t}"
    );
    // `pin carton` still writes the project's file, and says the lock wins
    let o = run(&fx, &[], &["pin", "carton", "0.1.1"]);
    assert!(text(&o).contains("the lock wins"), "{}", text(&o));
    // a script's header does not move it either
    std::fs::write(fx.project.join("s.cig"), "# cig >=1.21.0\n").unwrap();
    let o = run(&fx, &[], &["which", "cig", "--script", "s.cig"]);
    let t = text(&o);
    assert_eq!(o.status.code(), Some(0), "{t}");
    assert!(t.contains("cig 1.20.0, from carton 0.1.0"), "{t}");
}

#[test]
fn an_unlocked_carton_value_is_a_machine_wide_pin_below_a_projects_own() {
    let fx = two_cartons();
    layer(&fx, "carton = \"0.1.0\"\n");
    // no project pin: the machine layer's pin wins over the machine's own carton (0.1.1)
    let t = which(&fx, &[]);
    assert!(
        t.contains("from carton 0.1.0") && t.contains("the carton setting"),
        "{t}"
    );
    // a project's pin is closer, and wins
    std::fs::write(fx.project.join(".cig-carton"), "0.1.1\n").unwrap();
    let t = which(&fx, &[]);
    assert!(
        t.contains("from carton 0.1.1 (pinned in .cig-carton)"),
        "{t}"
    );
    // the person's own file moves an unlocked machine value
    persons_file(&fx, "carton = 0.1.1\n");
    std::fs::remove_file(fx.project.join(".cig-carton")).unwrap();
    assert!(which(&fx, &[]).contains("from carton 0.1.1"));
}

#[test]
fn a_first_run_answer_for_a_locked_setting_is_not_asked_and_not_written() {
    let fx = fixture();
    layer(&fx, "preset = \"strict\"\nlocked = [\"preset\"]\n");
    let o = run(&fx, &[], &["--json", "install", "--dry-run"]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    let d = data(&o);
    let q = d["questions"].as_array().unwrap();
    let preset = q
        .iter()
        .find(|q| q["id"] == "preset")
        .expect("preset question");
    assert_eq!(preset["status"], "already-set", "{preset}");
}
