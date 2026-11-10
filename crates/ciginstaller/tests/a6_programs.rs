// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! #30: `ciginstaller programs` finds compiled programs by their trailer on `PATH` and in the
//! project's folders and nowhere else, says which sit on a cig an advisory covers, and
//! `programs rebuild` swaps one for a verified rebuild or leaves it exactly as it was.
//!
//! The programs are bytes laid out as cig documents (`cig_compat::program::build`); cigcompile is
//! the fixture's fake, a shell script, so these tests are Unix only.
#![cfg(unix)]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Output;

use cig_compat::program;
use cigi_testkit::{sha256_hex, Fixture};
use common::{cigi, fixture, ok, text};
use serde_json::Value;

const SCRIPT: &str = "say \"hi\"\n";

/// cig 1.20.1 and a fake cigcompile whose build copies `$HOME/new-program` to `--out` and whose
/// verify and test fail when `$HOME/verify-fails` / `$HOME/test-fails` exist. Every call is logged
/// to `$HOME/cc-calls`. An advisory covers `cig <1.20.1`.
fn setup() -> Fixture {
    let mut fx = fixture();
    fx.product("cig", "1.20.1");
    fx.product("cigcompile", "0.2.0").shell(
        r#"echo "$*" >> "$HOME/cc-calls"
case "$1" in
build)
  out=; prev=
  for a in "$@"; do if [ "$prev" = "--out" ]; then out="$a"; fi; prev="$a"; done
  cp "$HOME/new-program" "$out" && chmod 755 "$out"; exit $?;;
verify) if [ -f "$HOME/verify-fails" ]; then echo "runtime: differs from the pinned release" >&2; exit 1; fi; exit 0;;
test) if [ -f "$HOME/test-fails" ]; then echo "plan differs from the script" >&2; exit 1; fi; exit 0;;
esac"#,
    );
    fx.carton("0.1.0", &[("cig", "1.20.1"), ("cigcompile", "0.2.0")]);
    fx.publish("stable");
    advise(&fx);
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    fx
}

fn advise(fx: &Fixture) {
    let path = fx.releases.join("manifest-stable.toml");
    let mut t = std::fs::read_to_string(&path).expect("manifest");
    t.push_str(
        "\n[[advisory]]\nid = \"CIGSA-2026-003\"\naffects = [ \"cig <1.20.1\" ]\nfixed_in = \"1.20.1\"\n\
         severity = \"security\"\nsummary = \"a program could skip a rollback\"\n",
    );
    std::fs::write(&path, &t).expect("write manifest");
    std::fs::write(
        fx.releases.join("manifest-stable.toml.sha256"),
        format!("{}  manifest-stable.toml\n", sha256_hex(t.as_bytes())),
    )
    .expect("write sidecar");
}

fn put(path: &Path, bytes: &[u8], mode: u32) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

fn program(name: &str, file: &str, cig: &str) -> Vec<u8> {
    program::build(
        b"\x7fELF-not-really-a-runtime",
        name,
        file,
        SCRIPT,
        cig,
        "cigcompile 0.2.0",
    )
}

fn pathbin(fx: &Fixture) -> PathBuf {
    fx.tmp.path().join("pathbin")
}

fn run(fx: &Fixture, args: &[&str]) -> Output {
    run_in(fx, &fx.project, args)
}

fn run_in(fx: &Fixture, cwd: &Path, args: &[&str]) -> Output {
    let path = format!(
        "{}:{}:/usr/bin:/bin",
        pathbin(fx).display(),
        fx.cigscript().join("bin").display()
    );
    let mut c = fx.cigi();
    c.env("CIGI_ALLOW_SINGLE_SOURCE", "true")
        .env("PATH", path)
        .current_dir(cwd)
        .arg("--yes")
        .args(args);
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

fn names(d: &Value) -> Vec<String> {
    let mut v: Vec<String> = d["programs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap().to_string())
        .collect();
    v.sort();
    v
}

fn pin_project(fx: &Fixture) {
    std::fs::write(fx.project.join(".cig-version"), "1.20.1\n").unwrap();
}

// ------------------------------------------------------------------ finding

#[test]
fn programs_are_found_on_path_and_in_a_project_folder_and_nowhere_else() {
    let fx = setup();
    pin_project(&fx);
    put(
        &pathbin(&fx).join("tidy"),
        &program("tidy", "tidy.cig", "1.20.0"),
        0o755,
    );
    // the project's folder and its bin/
    put(
        &fx.project.join("site"),
        &program("site", "site.cig", "1.20.1"),
        0o755,
    );
    put(
        &fx.project.join("bin/deploy"),
        &program("deploy", "d.cig", "1.20.1"),
        0o755,
    );
    // not looked in: another folder, a folder under the project, a program that is not runnable
    let elsewhere = fx.tmp.path().join("elsewhere");
    put(
        &elsewhere.join("stray"),
        &program("stray", "s.cig", "1.20.1"),
        0o755,
    );
    put(
        &fx.project.join("deep/er/buried"),
        &program("buried", "b.cig", "1.20.1"),
        0o755,
    );
    put(
        &pathbin(&fx).join("not-runnable"),
        &program("not-runnable", "n.cig", "1.20.1"),
        0o644,
    );
    put(
        &pathbin(&fx).join("plain.txt"),
        b"just some text, long enough to hold a trailer if it had one",
        0o755,
    );

    let o = run(&fx, &["--json", "programs"]);
    ok(&o);
    let d = data(&o);
    assert_eq!(names(&d), ["deploy", "site", "tidy"], "{d:#}");
    let looked = d["looked"]["projects"].as_array().unwrap();
    assert_eq!(looked.len(), 1, "{d:#}");
    assert!(looked[0].as_str().unwrap().ends_with("project"), "{d:#}");

    // a folder is looked in only when it is named
    let o = run(
        &fx,
        &[
            "--json",
            "programs",
            "--project",
            elsewhere.to_str().unwrap(),
        ],
    );
    ok(&o);
    assert_eq!(names(&data(&o)), ["deploy", "site", "stray", "tidy"]);

    // outside any project, the project's programs are not looked for
    let o = run_in(&fx, fx.tmp.path(), &["--json", "programs"]);
    ok(&o);
    assert_eq!(names(&data(&o)), ["tidy"]);
}

#[test]
fn a_program_is_listed_with_its_cig_its_compiler_and_where_it_is() {
    let fx = setup();
    let p = pathbin(&fx).join("tidy");
    put(&p, &program("tidy", "tools/tidy.cig", "1.20.1"), 0o755);
    let d = data(&run(&fx, &["--json", "programs"]));
    let t = &d["programs"][0];
    assert_eq!(t["name"], "tidy");
    assert_eq!(t["cig"], "1.20.1");
    assert_eq!(t["compiler"], "cigcompile 0.2.0");
    assert_eq!(t["script"], "tools/tidy.cig");
    assert_eq!(t["path"], p.to_str().unwrap());
    assert_eq!(d["cig"], "1.20.1");
    assert_eq!(t["needs_rebuild"], false);
    let o = run(&fx, &["--plain", "programs"]);
    ok(&o);
    let t = text(&o);
    assert!(t.contains("1 program") && t.contains("1.20.1"), "{t}");
}

#[test]
fn nothing_found_says_where_it_looked() {
    let fx = setup();
    let o = run(&fx, &["--plain", "programs"]);
    ok(&o);
    let t = text(&o);
    assert!(
        t.contains("no compiled programs found") && t.contains("nothing else is scanned"),
        "{t}"
    );
}

#[test]
fn a_malformed_or_truncated_trailer_is_skipped_with_a_note_never_a_panic() {
    let fx = setup();
    let good = program("good", "g.cig", "1.20.1");
    put(&pathbin(&fx).join("good"), &good, 0o755);
    // a flipped byte in the payload
    let mut flipped = program("flipped", "f.cig", "1.20.1");
    let n = flipped.len();
    flipped[n - 70] ^= 0x55;
    put(&pathbin(&fx).join("flipped"), &flipped, 0o755);
    // cut in the middle of the payload: the trailer is gone from the end
    let whole = program("cut", "c.cig", "1.20.1");
    put(&pathbin(&fx).join("cut"), &whole[..whole.len() - 80], 0o755);
    // a trailer that claims more than the file holds
    let mut liar = program("liar", "l.cig", "1.20.1");
    let n = liar.len();
    liar[n - 40..n - 32].copy_from_slice(&u64::MAX.to_le_bytes());
    put(&pathbin(&fx).join("liar"), &liar, 0o755);
    // a trailer alone, and only magic
    let mut alone = program::MAGIC.to_vec();
    alone.extend_from_slice(&[9u8; 40]);
    put(&pathbin(&fx).join("alone"), &alone, 0o755);
    // a record that lies about its name
    put(
        &pathbin(&fx).join("badname"),
        &program("../../evil", "e.cig", "1.20.1"),
        0o755,
    );

    let o = run(&fx, &["--json", "programs"]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    let d = data(&o);
    assert_eq!(names(&d), ["good"], "{d:#}");
    let notes: Vec<&str> = d["notes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap())
        .collect();
    for bad in ["flipped", "liar", "alone", "badname"] {
        assert!(
            notes
                .iter()
                .any(|n| n.contains(bad) && n.contains("skipped")),
            "no note for {bad}: {notes:?}"
        );
    }
    // the cut one has no trailer left to read: it is not a program this reader sees, and not a crash
    assert!(!text(&o).contains("panicked"), "{}", text(&o));
    let o = run(&fx, &["--plain", "programs"]);
    ok(&o);
    assert!(text(&o).contains("was skipped"), "{}", text(&o));
}

// ------------------------------------------------------------------ advisories

#[test]
fn an_advisory_on_the_programs_cig_is_shown_with_its_severity_and_fix() {
    let fx = setup();
    pin_project(&fx);
    put(
        &pathbin(&fx).join("tidy"),
        &program("tidy", "tidy.cig", "1.20.0"),
        0o755,
    );
    put(
        &pathbin(&fx).join("fresh"),
        &program("fresh", "fresh.cig", "1.20.1"),
        0o755,
    );

    let o = run(&fx, &["--json", "programs"]);
    ok(&o);
    let d = data(&o);
    let tidy = d["programs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "tidy")
        .unwrap();
    assert_eq!(tidy["needs_rebuild"], true);
    assert_eq!(tidy["advisories"][0]["id"], "CIGSA-2026-003");
    assert_eq!(tidy["advisories"][0]["severity"], "security");
    assert_eq!(tidy["advisories"][0]["fixed_in"], "1.20.1");
    let fresh = d["programs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "fresh")
        .unwrap();
    assert!(fresh["advisories"].as_array().unwrap().is_empty());
    let o = run(&fx, &["--plain", "programs"]);
    let t = text(&o);
    assert!(
        t.contains("1 needs a rebuild") && t.contains("fixed in 1.20.1"),
        "{t}"
    );
    assert!(t.contains("ciginstaller programs rebuild tidy"), "{t}");

    // status says it, once per program, and carries it in its JSON
    let o = run(&fx, &["--plain", "status"]);
    ok(&o);
    let t = text(&o);
    assert_eq!(t.matches("program tidy").count(), 1, "{t}");
    assert!(
        t.contains("CIGSA-2026-003") && !t.contains("program fresh"),
        "{t}"
    );
    let s = data(&run(&fx, &["--json", "status"]));
    assert_eq!(s["waiting"]["programs"][0]["name"], "tidy", "{s:#}");

    // doctor has a finding with the fix; a note, so its exit code is not changed by it
    let o = run(&fx, &["--json", "doctor"]);
    let d = data(&o);
    let f = d["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["section"] == "programs")
        .unwrap_or_else(|| panic!("no programs finding: {d:#}"));
    assert_eq!(f["fix"], "ciginstaller programs rebuild tidy");
    assert_eq!(f["severity"], "note");
}

#[test]
fn a_machine_without_a_program_advisory_reads_as_before() {
    let fx = setup();
    put(
        &pathbin(&fx).join("fresh"),
        &program("fresh", "fresh.cig", "1.20.1"),
        0o755,
    );
    let s = data(&run(&fx, &["--json", "status"]));
    assert!(s["waiting"].get("programs").is_none(), "{s:#}");
}

// ------------------------------------------------------------------ rebuild

/// A program on cig 1.20.0 on PATH whose script is at `<project>/src/tidy.cig`, and the rebuilt
/// program the fake cigcompile will write.
fn with_old_program(fx: &Fixture) -> (PathBuf, Vec<u8>, Vec<u8>) {
    let script = fx.project.join("src/tidy.cig");
    std::fs::create_dir_all(script.parent().unwrap()).unwrap();
    std::fs::write(&script, SCRIPT).unwrap();
    let old = program("tidy", script.to_str().unwrap(), "1.20.0");
    let new = program("tidy", script.to_str().unwrap(), "1.20.1");
    let p = pathbin(fx).join("tidy");
    put(&p, &old, 0o755);
    std::fs::write(fx.user_home.join("new-program"), &new).unwrap();
    (p, old, new)
}

fn calls(fx: &Fixture) -> Vec<String> {
    std::fs::read_to_string(fx.user_home.join("cc-calls"))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn a_rebuild_whose_checks_pass_swaps_the_program() {
    let fx = setup();
    let (p, old, new) = with_old_program(&fx);
    let o = run(&fx, &["--json", "programs", "rebuild", "tidy"]);
    ok(&o);
    let d = data(&o);
    assert_eq!(d["rebuilt"], true, "{d:#}");
    assert_eq!(d["cig_from"], "1.20.0");
    assert_eq!(d["cig_to"], "1.20.1");
    assert_eq!(d["advisories_cleared"][0], "CIGSA-2026-003");
    assert_ne!(old, new);
    assert_eq!(
        std::fs::read(&p).unwrap(),
        new,
        "the program was not swapped"
    );
    assert_eq!(
        std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
        0o755
    );
    // build, then verify, then the program's own test, all against the installed cig
    let c = calls(&fx);
    assert_eq!(c.len(), 3, "{c:?}");
    assert!(
        c[0].starts_with("build ") && c[0].contains("--runtime") && c[0].contains("src/tidy.cig"),
        "{c:?}"
    );
    assert!(
        c[1].starts_with("verify ") && c[1].contains("--script"),
        "{c:?}"
    );
    assert!(c[2].starts_with("test "), "{c:?}");
    // no staging left behind, and the history says so
    let st: Value = serde_json::from_str(
        &std::fs::read_to_string(fx.cigscript().join("installer/state.json")).unwrap(),
    )
    .unwrap();
    assert!(
        st["history"]
            .as_array()
            .unwrap()
            .iter()
            .any(|h| h["what"] == "programs"),
        "{st:#}"
    );
    // it no longer carries an advisory
    let d = data(&run(&fx, &["--json", "programs"]));
    assert_eq!(d["programs"][0]["cig"], "1.20.1");
    assert_eq!(d["programs"][0]["needs_rebuild"], false);
    // and asking again has nothing to do
    let o = run(&fx, &["--json", "programs", "rebuild", "tidy"]);
    ok(&o);
    assert_eq!(data(&o)["rebuilt"], false);
}

#[test]
fn a_failing_verify_leaves_the_old_program_in_place() {
    let fx = setup();
    let (p, old, _) = with_old_program(&fx);
    std::fs::write(fx.user_home.join("verify-fails"), "").unwrap();
    let o = run(&fx, &["programs", "rebuild", "tidy"]);
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
    let t = text(&o);
    assert!(
        t.contains("E1859") && t.contains("cigcompile verify") && t.contains("as it was"),
        "{t}"
    );
    assert_eq!(std::fs::read(&p).unwrap(), old, "the old program changed");
    assert!(
        !calls(&fx).iter().any(|c| c.starts_with("test ")),
        "the test ran after a failed verify"
    );
}

#[test]
fn a_failing_program_test_leaves_the_old_program_in_place() {
    let fx = setup();
    let (p, old, _) = with_old_program(&fx);
    std::fs::write(fx.user_home.join("test-fails"), "").unwrap();
    let o = run(&fx, &["programs", "rebuild", "tidy"]);
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
    assert!(text(&o).contains("cigcompile test"), "{}", text(&o));
    assert_eq!(std::fs::read(&p).unwrap(), old);
}

#[test]
fn a_rebuilt_program_that_is_not_what_was_asked_for_is_refused() {
    let fx = setup();
    let (p, old, _) = with_old_program(&fx);
    // the fake builds a program for the wrong cig
    let wrong = program(
        "tidy",
        fx.project.join("src/tidy.cig").to_str().unwrap(),
        "1.20.0",
    );
    std::fs::write(fx.user_home.join("new-program"), wrong).unwrap();
    let o = run(&fx, &["programs", "rebuild", "tidy"]);
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
    assert!(
        text(&o).contains("not the installed 1.20.1"),
        "{}",
        text(&o)
    );
    assert_eq!(std::fs::read(&p).unwrap(), old);
}

#[test]
fn a_missing_script_is_said_and_the_advisory_listed_for_the_author() {
    let fx = setup();
    let old = program("tidy", "/nowhere/at/all/tidy.cig", "1.20.0");
    let p = pathbin(&fx).join("tidy");
    put(&p, &old, 0o755);
    let o = run(&fx, &["programs", "rebuild", "tidy"]);
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
    let t = text(&o);
    assert!(
        t.contains("E1859") && t.contains("script of tidy is not on this machine"),
        "{t}"
    );
    assert!(t.contains("CIGSA-2026-003") && t.contains("author"), "{t}");
    assert_eq!(std::fs::read(&p).unwrap(), old);
    assert!(calls(&fx).is_empty(), "cigcompile ran without a script");
    // `--script` names it
    let script = fx.tmp.path().join("kept/tidy.cig");
    std::fs::create_dir_all(script.parent().unwrap()).unwrap();
    std::fs::write(&script, SCRIPT).unwrap();
    std::fs::write(
        fx.user_home.join("new-program"),
        program("tidy", script.to_str().unwrap(), "1.20.1"),
    )
    .unwrap();
    let o = run(
        &fx,
        &[
            "programs",
            "rebuild",
            "tidy",
            "--script",
            script.to_str().unwrap(),
        ],
    );
    ok(&o);
}

#[test]
fn rebuild_names_what_it_cannot_find_or_cannot_tell_apart() {
    let fx = setup();
    let o = run(&fx, &["programs", "rebuild", "ghost"]);
    assert_eq!(o.status.code(), Some(3), "{}", text(&o));
    assert!(text(&o).contains("E1870"), "{}", text(&o));
    let a = pathbin(&fx).join("tidy");
    let b = fx.tmp.path().join("other/tidy");
    put(&a, &program("tidy", "t.cig", "1.20.0"), 0o755);
    put(&b, &program("tidy", "t.cig", "1.20.0"), 0o755);
    let o = run(
        &fx,
        &[
            "programs",
            "rebuild",
            "tidy",
            "--project",
            b.parent().unwrap().to_str().unwrap(),
        ],
    );
    assert_eq!(o.status.code(), Some(3), "{}", text(&o));
    assert!(text(&o).contains("names 2 programs"), "{}", text(&o));
}

#[test]
fn a_dry_run_builds_nothing_and_swaps_nothing() {
    let fx = setup();
    let (p, old, _) = with_old_program(&fx);
    let o = run(&fx, &["--json", "programs", "rebuild", "tidy", "--dry-run"]);
    ok(&o);
    assert_eq!(data(&o)["dry_run"], true);
    assert_eq!(std::fs::read(&p).unwrap(), old);
    assert!(calls(&fx).is_empty());
}

#[test]
fn a_setuid_program_is_not_replaced() {
    let fx = setup();
    let (p, old, _) = with_old_program(&fx);
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o4755)).unwrap();
    let o = run(&fx, &["programs", "rebuild", "tidy"]);
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
    assert!(text(&o).contains("raised rights"), "{}", text(&o));
    assert_eq!(std::fs::read(&p).unwrap(), old);
}

#[test]
fn a_link_is_followed_to_the_program_it_names() {
    let fx = setup();
    let (p, _, new) = with_old_program(&fx);
    let real = fx.tmp.path().join("real/tidy");
    std::fs::create_dir_all(real.parent().unwrap()).unwrap();
    std::fs::rename(&p, &real).unwrap();
    std::os::unix::fs::symlink(&real, &p).unwrap();
    let o = run(&fx, &["--json", "programs", "rebuild", "tidy"]);
    ok(&o);
    assert!(
        std::fs::symlink_metadata(&p)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the link was replaced"
    );
    assert_eq!(std::fs::read(&real).unwrap(), new);
}

// ------------------------------------------------------------------ #19

#[test]
fn programs_never_makes_a_request() {
    let fx = setup();
    let (_, _, _) = with_old_program(&fx);
    let server = fx.serve();
    let path = format!("{}:/usr/bin:/bin", pathbin(&fx).display());
    for args in [
        &["--json", "programs"][..],
        &["--json", "programs", "rebuild", "tidy", "--dry-run"][..],
        &["--json", "status"][..],
        &["--json", "doctor"][..],
        &["--json", "programs", "rebuild", "tidy"][..],
    ] {
        let mut c = fx.cigi_with_mirror(&server.url());
        c.env("CIGI_ALLOW_SINGLE_SOURCE", "true")
            .env("PATH", &path)
            .arg("--yes")
            .args(args);
        let o = cigi_testkit::output_retrying(&mut c).expect("run");
        assert!(o.status.code().is_some(), "{}", text(&o));
    }
    assert!(
        server.requests().is_empty(),
        "requests were made: {:?}",
        server.paths()
    );
    assert!(server.assert_no_telemetry().is_ok());
}
