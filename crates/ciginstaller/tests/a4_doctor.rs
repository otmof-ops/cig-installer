// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! Phase 4 acceptance (SPEC 14.3): `doctor`. Each finding is produced by a fault, ends with a
//! command, and that command, when run, clears it (the test runs it and asks doctor again).
//! Assertions are on codes, sections, severities and `--json` data, not on glyphs or labels.

mod common;

use cigi_testkit::{Fixture, Snapshot};
use common::{cigi, fixture, ok, text};
use serde_json::Value;

/// `Command::output`, tried again while exec fails with "text file busy" (ETXTBSY).
fn retrying(c: &mut std::process::Command) -> std::process::Output {
    for _ in 0..200 {
        match c.output() {
            Err(e) if e.raw_os_error() == Some(26) => {
                std::thread::sleep(std::time::Duration::from_millis(5))
            }
            r => return r.expect("run doctor"),
        }
    }
    panic!("text file busy for a second");
}

fn setup() -> Fixture {
    let mut fx = fixture();
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.16.0")]);
    fx.carton("0.2.0", &[("cig", "1.21.0"), ("cigo", "1.17.0")]);
    fx.publish("stable");
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    fx
}

/// `(exit code, the data object of --json doctor)`.
fn doctor_args(fx: &Fixture, path: Option<&str>, extra: &[&str]) -> (i32, Value) {
    let mut c = fx.cigi();
    c.env("CIGI_ALLOW_SINGLE_SOURCE", "true")
        .arg("--yes")
        .arg("--json")
        .arg("doctor")
        .args(extra);
    if let Some(p) = path {
        c.env("PATH", p);
    }
    let o = retrying(&mut c);
    let line = String::from_utf8_lossy(&o.stdout)
        .lines()
        .last()
        .unwrap_or("")
        .to_string();
    let v: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {}", text(&o)));
    (o.status.code().unwrap_or(-1), v["data"].clone())
}

fn doctor(fx: &Fixture) -> (i32, Value) {
    doctor_args(fx, None, &[])
}

fn findings<'a>(d: &'a Value, section: &str, sev: &str) -> Vec<&'a Value> {
    d["findings"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|f| f["section"] == section && f["severity"] == sev)
                .collect()
        })
        .unwrap_or_default()
}

fn clean(fx: &Fixture) {
    let (code, d) = doctor(fx);
    let acts: Vec<&Value> = d["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["severity"] == "action")
        .collect();
    assert!(acts.is_empty(), "still needs action: {acts:#?}");
    assert_eq!(code, 0);
}

/// Run the fix command a finding printed (`ciginstaller <args>`).
fn run_fix(fx: &Fixture, fix: &str) {
    let rest = fix.strip_prefix("ciginstaller ").expect(fix);
    let args: Vec<&str> = rest.split_whitespace().collect();
    ok(&cigi(fx, &args));
}

#[test]
fn a4_doctor_clean_home_is_read_only() {
    let fx = setup();
    let before = Snapshot::take(&fx.user_home);
    let (code, d) = doctor(&fx);
    assert_eq!(code, 0, "{d:#}");
    assert_eq!(d["needs_action"], 0);
    for s in [
        "transactions",
        "toolchains",
        "index",
        "shims",
        "PATH",
        "PATH block",
        "humidor",
        "disk",
    ] {
        assert!(!findings(&d, s, "ok").is_empty(), "no ok finding for {s}");
    }
    // the sections the build does not have are said to be absent
    assert!(!findings(&d, "not in this build", "note").is_empty());
    // every finding has a because; every action has a fix
    for f in d["findings"].as_array().unwrap() {
        assert!(!f["because"].as_str().unwrap_or("").is_empty(), "{f}");
    }
    // no lock, no journal, no cache write: the home is exactly as it was
    Snapshot::take(&fx.user_home).assert_same(&before);
    // the tools' own doctors are part of the default report (the fakes have no self-check)
    assert_eq!(d["tools"].as_array().unwrap().len(), 2, "{d:#}");
}

#[test]
fn a4_doctor_nothing_installed() {
    let fx = fixture();
    let (code, d) = doctor(&fx);
    assert_eq!(code, 0);
    assert_eq!(d["needs_action"], 0);
    let n = findings(&d, "install", "note");
    assert_eq!(n.len(), 1);
    assert_eq!(n[0]["fix"], "ciginstaller install carton");
    assert!(!fx.cigscript().exists(), "doctor made the home");
}

#[test]
fn a4_doctor_deleted_shim() {
    let fx = setup();
    std::fs::remove_file(fx.cigscript().join("bin/cigo")).unwrap();
    let (code, d) = doctor(&fx);
    assert_eq!(code, 2, "{d:#}");
    let f = findings(&d, "shims", "action");
    assert_eq!(f.len(), 1, "{d:#}");
    assert_eq!(f[0]["code"], "E1844");
    assert!(f[0]["found"].as_str().unwrap().contains("cigo"));
    run_fix(&fx, f[0]["fix"].as_str().unwrap());
    clean(&fx);
    assert!(fx.cigscript().join("bin/cigo").exists());
}

#[test]
fn a4_doctor_tampered_toolchain_file() {
    let fx = setup();
    let file = fx.cigscript().join("toolchains/cigo/1.16.0/bin/cigo");
    let original = std::fs::read(&file).unwrap();
    // a tool that replaces its file by rename breaks only that link (SPEC 4.3)
    let tmp = file.with_file_name("cigo.new");
    std::fs::write(&tmp, b"#!/bin/sh\necho tampered\n").unwrap();
    std::fs::rename(&tmp, &file).unwrap();
    let (code, d) = doctor(&fx);
    assert_eq!(code, 2, "{d:#}");
    let f = findings(&d, "toolchains", "action");
    assert_eq!(f.len(), 1, "{d:#}");
    assert_eq!(f[0]["code"], "E1831");
    assert!(f[0]["found"].as_str().unwrap().contains("bin/cigo"));
    run_fix(&fx, f[0]["fix"].as_str().unwrap());
    clean(&fx);
    assert_eq!(std::fs::read(&file).unwrap(), original);
}

#[test]
fn a4_doctor_hand_edited_index() {
    let fx = setup();
    let idx = fx.cigscript().join("installer/shim-index");
    let original = std::fs::read(&idx).unwrap();
    let mut edited = original.clone();
    edited.extend_from_slice(b"pin\t9.9\tcigo\t/nowhere\n");
    std::fs::write(&idx, &edited).unwrap();
    let (code, d) = doctor(&fx);
    assert_eq!(code, 2, "{d:#}");
    let f = findings(&d, "index", "action");
    assert_eq!(f.len(), 1, "{d:#}");
    run_fix(&fx, f[0]["fix"].as_str().unwrap());
    clean(&fx);
    assert_eq!(
        std::fs::read(&idx).unwrap(),
        original,
        "regenerated byte for byte"
    );
    // a missing index is the same finding
    std::fs::remove_file(&idx).unwrap();
    let (_, d) = doctor(&fx);
    let f = findings(&d, "index", "action");
    assert_eq!(f.len(), 1, "{d:#}");
    assert_eq!(f[0]["fix"], "ciginstaller shims sync");
    run_fix(&fx, f[0]["fix"].as_str().unwrap());
    assert_eq!(std::fs::read(&idx).unwrap(), original);
}

#[test]
fn a4_doctor_interrupted_transaction() {
    let fx = setup();
    let o = fx
        .cigi()
        .env("CIGI_ALLOW_SINGLE_SOURCE", "true")
        .env("CIGI_FAULT", "link:3")
        .args(["--yes", "install", "carton", "--carton", "0.2.0"])
        .output()
        .unwrap();
    assert!(
        o.status.code().is_none(),
        "the install should have been killed"
    );
    let before = Snapshot::take(&fx.user_home);
    let (code, d) = doctor(&fx);
    assert_eq!(code, 2, "{d:#}");
    let f = findings(&d, "transactions", "action");
    assert_eq!(f.len(), 1, "{d:#}");
    assert_eq!(f[0]["code"], "E1832");
    // doctor only looks: it did not settle the transaction
    Snapshot::take(&fx.user_home).assert_same(&before);
    let o = cigi(&fx, &["repair"]);
    ok(&o);
    assert_eq!(text(&o).matches("E1832").count(), 1, "{}", text(&o));
    clean(&fx);
    assert!(!fx.cigscript().join("toolchains/cigo/1.17.0").exists());
}

// Unix only: a shell-script shadow earlier on PATH is reported with E1844 (PATH is `:`-separated).
#[cfg(unix)]
#[test]
fn a4_doctor_shadowing_binary_and_path() {
    let fx = setup();
    let bin = fx.cigscript().join("bin");
    let shadow = fx.tmp.path().join("shadow");
    std::fs::create_dir_all(&shadow).unwrap();
    let fake = shadow.join("cigo");
    std::fs::write(&fake, "#!/bin/sh\necho not ours\n").unwrap();
    std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let bad = format!("{}:{}:/usr/bin:/bin", shadow.display(), bin.display());
    let (code, d) = doctor_args(&fx, Some(&bad), &[]);
    assert_eq!(code, 2, "{d:#}");
    let f = findings(&d, "PATH", "action");
    assert_eq!(f.len(), 1, "{d:#}");
    assert_eq!(f[0]["code"], "E1844");
    assert!(f[0]["found"].as_str().unwrap().contains("cigo"));
    let fix = f[0]["fix"].as_str().unwrap().to_string();
    assert!(fix.starts_with("export PATH="), "{fix}");
    assert!(fix.contains(&bin.display().to_string()), "{fix}");
    // the fix puts bin first: apply it the way a shell would
    let fixed = format!("{}:{bad}", bin.display());
    let (code, d) = doctor_args(&fx, Some(&fixed), &[]);
    assert_eq!(code, 0, "{d:#}");
    // not on PATH at all
    let (code, d) = doctor_args(&fx, Some("/usr/bin:/bin"), &[]);
    assert_eq!(code, 2);
    assert_eq!(findings(&d, "PATH", "action")[0]["code"], "E1845");
}

#[test]
fn a4_doctor_path_block() {
    let fx = setup();
    let profile = fx.user_home.join(".profile");
    let original = std::fs::read_to_string(&profile).unwrap();
    assert!(original.contains("# >>> cigscript >>>"));
    // the block was deleted by hand: I know it was agreed to, so repair puts it back
    std::fs::write(&profile, "export FOO=1\n").unwrap();
    let (code, d) = doctor(&fx);
    assert_eq!(code, 2, "{d:#}");
    let f = findings(&d, "PATH block", "action");
    assert_eq!(f.len(), 1, "{d:#}");
    assert_eq!(f[0]["code"], "E1845");
    assert_eq!(f[0]["fix"], "ciginstaller repair");
    run_fix(&fx, "ciginstaller repair");
    clean(&fx);
    let t = std::fs::read_to_string(&profile).unwrap();
    assert!(t.starts_with("export FOO=1\n"), "{t}");
    assert_eq!(t.matches("# >>> cigscript >>>").count(), 1, "{t}");
    // an edited block is left alone, by repair too
    let edited = t.replace("export PATH=", "export  PATH=");
    std::fs::write(&profile, &edited).unwrap();
    let (code, d) = doctor(&fx);
    assert_eq!(code, 2);
    assert_eq!(findings(&d, "PATH block", "action")[0]["code"], "E1862");
    ok(&cigi(&fx, &["repair"]));
    assert_eq!(std::fs::read_to_string(&profile).unwrap(), edited);
}

#[test]
fn a4_doctor_respects_no_path() {
    let mut fx = fixture();
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.16.0")]);
    fx.publish("stable");
    ok(&cigi(&fx, &["install", "carton", "--no-path"]));
    let (code, d) = doctor(&fx);
    assert_eq!(code, 0, "{d:#}");
    assert_eq!(findings(&d, "PATH block", "note").len(), 1);
    assert!(findings(&d, "PATH block", "action").is_empty());
    let o = cigi(&fx, &["repair"]);
    ok(&o);
    assert!(
        !fx.user_home.join(".profile").exists(),
        "repair added a block that was declined"
    );
}

#[test]
fn a4_doctor_stale_and_expired_manifest() {
    let fx = setup();
    let meta = fx
        .cigscript()
        .join("installer/cache/manifests/stable/fetched.json");
    let original = std::fs::read_to_string(&meta).unwrap();
    let edit = |key: &str, to: &str| {
        let mut v: Value = serde_json::from_str(&original).unwrap();
        v[key] = Value::String(to.to_string());
        std::fs::write(&meta, v.to_string()).unwrap();
    };
    edit("fetched_at", "2020-01-01T00:00:00Z");
    let (code, d) = doctor(&fx);
    assert_eq!(code, 0, "an old cache is a note, not an action: {d:#}");
    let n = findings(&d, "manifest", "note");
    assert_eq!(n.len(), 1, "{d:#}");
    assert_eq!(n[0]["fix"], "ciginstaller update --check --refresh");
    run_fix(&fx, "ciginstaller update --check --refresh");
    let (_, d) = doctor(&fx);
    assert!(findings(&d, "manifest", "note").is_empty(), "{d:#}");
    std::fs::write(&meta, &original).unwrap();
    edit("expires", "2020-01-01T00:00:00Z");
    let (code, d) = doctor(&fx);
    assert_eq!(code, 2, "{d:#}");
    let f = findings(&d, "manifest", "action");
    assert_eq!(f[0]["code"], "E1823");
    run_fix(&fx, f[0]["fix"].as_str().unwrap());
    clean(&fx);
}

#[test]
fn a4_doctor_humidor_mismatch() {
    let fx = setup();
    let link = fx.cigscript().join("humidor/1.20.0/cig");
    assert!(link.exists());
    std::fs::remove_file(&link).unwrap();
    let (code, d) = doctor(&fx);
    assert_eq!(code, 2, "{d:#}");
    let f = findings(&d, "humidor", "action");
    assert_eq!(f.len(), 1, "{d:#}");
    run_fix(&fx, f[0]["fix"].as_str().unwrap());
    clean(&fx);
    assert!(link.exists());
    // and an entry that was dropped from the index
    let idx = fx.cigscript().join("humidor/index.json");
    let original = std::fs::read(&idx).unwrap();
    std::fs::write(&idx, "{\"entries\": []}\n").unwrap();
    let (_, d) = doctor(&fx);
    assert_eq!(findings(&d, "humidor", "action").len(), 1, "{d:#}");
    run_fix(&fx, "ciginstaller repair");
    clean(&fx);
    let back: Value = serde_json::from_slice(&std::fs::read(&idx).unwrap()).unwrap();
    let was: Value = serde_json::from_slice(&original).unwrap();
    assert_eq!(back["entries"][0]["version"], was["entries"][0]["version"]);
    assert_eq!(back["entries"][0]["sha256"], was["entries"][0]["sha256"]);
}

#[test]
fn a4_doctor_extra_file_in_a_toolchain() {
    let fx = setup();
    std::fs::write(fx.cigscript().join("toolchains/cig/1.20.0/bin/stray"), "x").unwrap();
    let (code, d) = doctor(&fx);
    assert_eq!(code, 2, "{d:#}");
    assert!(findings(&d, "toolchains", "action")[0]["found"]
        .as_str()
        .unwrap()
        .contains("stray"));
    run_fix(&fx, "ciginstaller repair");
    clean(&fx);
    assert!(!fx
        .cigscript()
        .join("toolchains/cig/1.20.0/bin/stray")
        .exists());
}

/// The tools' own checks are part of the default report, each is `<tool> __doctor --json`, and
/// `--tools` shows them and nothing else. The fakes here echo their arguments, which is not JSON:
/// they have no self-check (a6_family has the rest).
#[test]
fn a4_doctor_runs_the_tools_by_default_and_tools_means_only_them() {
    let fx = setup();
    let (_, both) = doctor(&fx);
    assert!(!both["findings"].as_array().unwrap().is_empty());
    let tools = both["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 2, "{both:#}");
    let (code, only) = doctor_args(&fx, None, &["--tools"]);
    assert_eq!(code, 0, "{only:#}");
    assert!(
        only["findings"].as_array().unwrap().is_empty(),
        "--tools printed the installer's own findings: {only:#}"
    );
    assert_eq!(only["tools"], both["tools"]);
    let cig = tools.iter().find(|t| t["tool"] == "cig 1.20.0").unwrap();
    assert_eq!(cig["self_check"], false);
    assert_eq!(
        cig["note"],
        "this cig build has no self-check; it needs a newer build"
    );
    assert!(!only.to_string().contains("args=doctor"));
}

/// cigcompile speaks the protocol: its own answer, in the shape `cigcompile __doctor --json`
/// prints (`{"tool","version","findings":[{check,severity,found,because,fix}]}`), is read and
/// shown under its heading, by default and under `--tools`.
#[test]
fn a4_doctor_reads_cigcompiles_own_self_check() {
    let mut fx = fixture();
    fx.product("cigcompile", "0.2.0").doctor_json(
        r#"{"tool":"cigcompile","version":"0.2.0","findings":[
  {"check":"runtime","severity":"ok","found":"cig 1.20.0 at the humidor","because":"a build for this machine appends its program to this cig","fix":null},
  {"check":"cache","severity":"action","found":"the build cache is damaged","because":"its index does not parse","fix":"cigcompile cache clear"}]}"#,
    );
    fx.carton(
        "0.1.0",
        &[
            ("cig", "1.20.0"),
            ("cigo", "1.16.0"),
            ("cigcompile", "0.2.0"),
        ],
    );
    fx.publish("stable");
    ok(&cigi(&fx, &["install", "carton", "--carton", "0.1.0"]));
    for extra in [&[][..], &["--tools"][..]] {
        let (code, d) = doctor_args(&fx, None, extra);
        let t = d["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["tool"] == "cigcompile 0.2.0")
            .unwrap_or_else(|| panic!("cigcompile is not named: {d:#}"));
        assert_eq!(t["self_check"], true, "{t}");
        let f = t["findings"].as_array().unwrap();
        assert_eq!(f.len(), 2, "{t}");
        assert_eq!(f[0]["section"], "runtime");
        assert_eq!(f[1]["severity"], "action");
        assert_eq!(f[1]["fix"], "cigcompile cache clear");
        // its action counts, and the first fix is the one it gave
        assert_eq!(code, 2, "{d:#}");
        assert!(d["needs_action"].as_u64().unwrap() >= 1);
    }
}

/// A fixture root for the kernel probes: `proc/...` and `sys/...` as the guard's features read
/// them. `None` leaves the file out.
fn kernel_root(
    fx: &Fixture,
    release: Option<&str>,
    userns: Option<&str>,
    lsm: Option<&str>,
) -> std::path::PathBuf {
    let root = fx.tmp.path().join("probe-root");
    let put = |rel: &str, body: Option<&str>| {
        if let Some(b) = body {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, format!("{b}\n")).unwrap();
        }
    };
    std::fs::create_dir_all(&root).unwrap();
    put("proc/sys/kernel/osrelease", release);
    put("proc/sys/user/max_user_namespaces", userns);
    put("sys/kernel/security/lsm", lsm);
    root
}

/// `(exit code, data)` of `--json doctor` with the probes pointed at `root`.
fn doctor_probing(fx: &Fixture, root: &std::path::Path) -> (i32, Value) {
    let mut c = fx.cigi();
    c.env("CIGI_ALLOW_SINGLE_SOURCE", "true")
        .env("CIGI_DOCTOR_PROBE_ROOT", root)
        .args(["--yes", "--json", "doctor"]);
    let o = retrying(&mut c);
    let line = String::from_utf8_lossy(&o.stdout)
        .lines()
        .last()
        .unwrap_or("")
        .to_string();
    let v: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {}", text(&o)));
    (o.status.code().unwrap_or(-1), v["data"].clone())
}

fn one<'a>(d: &'a Value, section: &str) -> &'a Value {
    let all: Vec<&Value> = d["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["section"] == section)
        .collect();
    assert_eq!(all.len(), 1, "{section}: {all:#?}");
    all[0]
}

#[test]
fn a4_doctor_guard_kernel_features_all_present() {
    let fx = setup();
    let root = kernel_root(
        &fx,
        Some("6.8.0-1-generic"),
        Some("31000"),
        Some("lockdown,capability,yama,apparmor,bpf"),
    );
    let (code, d) = doctor_probing(&fx, &root);
    assert_eq!(code, 0, "{d:#}");
    for s in ["kernel", "user namespaces", "BPF LSM"] {
        assert_eq!(one(&d, s)["severity"], "ok", "{s}: {d:#}");
    }
}

/// Each missing feature is a finding that ends with its one fix command; none is an action, so a
/// machine without a guard feature still has a doctor that exits 0.
#[test]
fn a4_doctor_guard_kernel_features_missing_each_end_with_a_fix() {
    let fx = setup();
    let root = kernel_root(
        &fx,
        Some("5.4.0-170-generic"),
        Some("0"),
        Some("lockdown,yama,apparmor"),
    );
    let (code, d) = doctor_probing(&fx, &root);
    assert_eq!(code, 0, "{d:#}");
    assert_eq!(d["needs_action"], 0);
    let kernel = one(&d, "kernel");
    assert_eq!(kernel["severity"], "note");
    assert!(kernel["found"]
        .as_str()
        .unwrap()
        .contains("5.4.0-170-generic"));
    assert!(kernel["found"].as_str().unwrap().contains("5.12"));
    let userns = one(&d, "user namespaces");
    assert_eq!(userns["severity"], "note");
    assert_eq!(
        userns["fix"],
        "sudo sysctl -w user.max_user_namespaces=15000"
    );
    let bpf = one(&d, "BPF LSM");
    assert_eq!(bpf["severity"], "note");
    let fix = bpf["fix"].as_str().unwrap();
    // the fix keeps the modules that are on and adds bpf
    assert!(fix.contains("lsm=lockdown,yama,apparmor,bpf"), "{fix}");
    assert!(fix.ends_with("sudo update-grub"), "{fix}");
}

#[test]
fn a4_doctor_guard_kernel_features_unreadable_say_so_and_never_pass() {
    let fx = setup();
    let root = kernel_root(&fx, None, None, None);
    let (code, d) = doctor_probing(&fx, &root);
    assert_eq!(code, 0, "{d:#}");
    for s in ["kernel", "user namespaces", "BPF LSM"] {
        assert_eq!(one(&d, s)["severity"], "note", "{s}: {d:#}");
        assert!(!one(&d, s)["because"].as_str().unwrap().is_empty());
    }
    // a kernel with no user namespaces has no file for a command to change
    assert!(one(&d, "user namespaces")["fix"].is_null());
}

/// The seam only changes where doctor reads: nothing in the folder or the home is written, and
/// without it the same sections come from the machine itself.
#[test]
fn a4_doctor_probe_root_is_only_a_place_to_read_from() {
    let fx = setup();
    let root = kernel_root(&fx, Some("6.8.0"), Some("1"), Some("bpf"));
    let before = Snapshot::take(&root);
    let home = Snapshot::take(&fx.user_home);
    let (_, d) = doctor_probing(&fx, &root);
    one(&d, "BPF LSM");
    Snapshot::take(&root).assert_same(&before);
    Snapshot::take(&fx.user_home).assert_same(&home);
    let (_, host) = doctor(&fx);
    for s in ["kernel", "user namespaces", "BPF LSM"] {
        one(&host, s);
    }
}

#[test]
fn a4_doctor_measures_the_tin_beside_the_store() {
    let fx = setup();
    // no tin yet
    let (_, d) = doctor(&fx);
    let t = one(&d, "tin");
    assert_eq!(t["severity"], "ok");
    assert!(t["found"].as_str().unwrap().contains("no tin"));
    // a small tin: measured, with cig tin gc as the fix, and no note
    let objects = fx.cigscript().join("tin/objects/ab");
    std::fs::create_dir_all(&objects).unwrap();
    std::fs::write(objects.join("cdef"), vec![7u8; 4096]).unwrap();
    let before = Snapshot::take(&fx.cigscript());
    let (code, d) = doctor(&fx);
    assert_eq!(code, 0, "{d:#}");
    let t = one(&d, "tin");
    assert_eq!(t["severity"], "ok");
    assert_eq!(t["fix"], "cig tin gc");
    let found = t["found"].as_str().unwrap();
    assert!(
        found.contains("the tin holds") && found.contains("1 file(s)"),
        "{found}"
    );
    assert!(found.contains("beside the store"), "{found}");
    Snapshot::take(&fx.cigscript()).assert_same(&before);
    // a big one is a note
    let big = std::fs::File::create(objects.join("big")).unwrap();
    big.set_len((1 << 30) + 1).unwrap();
    let (code, d) = doctor(&fx);
    assert_eq!(code, 0, "{d:#}");
    let t = one(&d, "tin");
    assert_eq!(t["severity"], "note", "{t}");
    assert_eq!(t["fix"], "cig tin gc");
}

#[test]
fn a4_doctor_voice_in_plain_mode() {
    let fx = setup();
    std::fs::remove_file(fx.cigscript().join("bin/cigo")).unwrap();
    let o = cigi(&fx, &["--plain", "doctor"]);
    assert_eq!(o.status.code(), Some(2));
    let t = text(&o);
    assert!(t.contains("because I checked"), "{t}");
    assert!(t.contains("ciginstaller repair"), "{t}");
    assert!(t.contains("E1844"), "{t}");
}
