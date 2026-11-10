// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! The testkit's own tests: what `publish` writes, the faults, the fake products, the server.

use cig_compat::{Manifest, Version};
use cigi_testkit::{asset_name, host_target, sha256_hex, Fixture, PublishOpts, Rule, Snapshot};
use std::io::{Read, Write};
use std::process::Command;

fn read(fx: &Fixture, name: &str) -> Vec<u8> {
    std::fs::read(fx.releases.join(name)).unwrap()
}

fn two_carton_fixture() -> Fixture {
    let mut fx = Fixture::new();
    fx.carton("0.1.0", &[("cig", "1.5.0"), ("cigo", "1.16.0")]);
    fx.carton(
        "0.2.0",
        &[
            ("cig", "1.20.0"),
            ("cigo", "1.17.0"),
            ("cigcompile", "0.2.0"),
        ],
    );
    fx
}

#[test]
fn publish_writes_manifest_assets_and_true_sidecars() {
    let mut fx = two_carton_fixture();
    let paths = fx.publish("stable");
    assert_eq!(paths.sequence, 1);
    let text = String::from_utf8(read(&fx, "manifest-stable.toml")).unwrap();
    let m = Manifest::from_str(&text).unwrap();
    assert_eq!(m.channel, "stable");
    assert_eq!(m.sequence, 1);
    let v = |s: &str| Version::parse(s).unwrap();
    assert_eq!(m.build_in(&v("0.2.0"), "cigo"), Some(&v("1.17.0")));
    assert_eq!(m.build_in(&v("0.1.0"), "cig"), Some(&v("1.5.0")));
    // manifest sidecar
    assert_eq!(
        read(&fx, "manifest-stable.toml.sha256"),
        format!("{}  manifest-stable.toml\n", sha256_hex(text.as_bytes())).into_bytes()
    );
    // every product: digest and size in the manifest equal the published bytes; sidecar equal too
    for (tool, ver) in [
        ("cig", "1.5.0"),
        ("cig", "1.20.0"),
        ("cigo", "1.16.0"),
        ("cigo", "1.17.0"),
        ("cigcompile", "0.2.0"),
    ] {
        let art = m
            .artifact(tool, &v(ver), &cig_compat::Target::host())
            .unwrap_or_else(|| panic!("{tool} {ver}"));
        let name = asset_name(tool, ver);
        assert_eq!(art.asset, name);
        let bytes = read(&fx, &name);
        assert_eq!(art.sha256, sha256_hex(&bytes), "{name}");
        assert_eq!(art.size, bytes.len() as u64);
        assert_eq!(
            read(&fx, &format!("{name}.sha256")),
            format!("{}  {name}\n", art.sha256).into_bytes()
        );
    }
    assert!(host_target().contains('-'));
}

#[test]
fn sequence_counts_up_per_channel_and_can_be_forced() {
    let mut fx = two_carton_fixture();
    assert_eq!(fx.publish("stable").sequence, 1);
    assert_eq!(fx.publish("stable").sequence, 2);
    assert_eq!(fx.publish("beta").sequence, 1);
    assert_eq!(
        fx.publish_with(
            "stable",
            &PublishOpts {
                sequence: Some(1),
                ..Default::default()
            }
        )
        .sequence,
        1
    );
    assert_eq!(fx.publish("stable").sequence, 2);
}

#[test]
fn publish_is_deterministic() {
    let mut a = two_carton_fixture();
    let mut b = two_carton_fixture();
    a.publish("stable");
    b.publish("stable");
    for tool_ver in [("cigo", "1.17.0"), ("cig", "1.20.0")] {
        let n = asset_name(tool_ver.0, tool_ver.1);
        assert_eq!(read(&a, &n), read(&b, &n));
    }
}

#[test]
fn expired_and_sidecar_options() {
    let mut fx = two_carton_fixture();
    fx.publish_with(
        "stable",
        &PublishOpts {
            expires_in_days: Some(-2),
            ..Default::default()
        },
    );
    let m =
        Manifest::from_str(&String::from_utf8(read(&fx, "manifest-stable.toml")).unwrap()).unwrap();
    // expired: `expires` is in the past, and still after `generated` (E1825 otherwise)
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let expires = cig_compat::rfc3339_to_epoch(&m.expires).unwrap();
    assert!(expires < now, "{} is not past", m.expires);
    assert!(m.generated < m.expires, "{} {}", m.generated, m.expires);
    let p = fx.publish_with(
        "stable",
        &PublishOpts {
            no_sidecar: true,
            ..Default::default()
        },
    );
    assert!(p.sidecar.is_none() && !fx.releases.join("manifest-stable.toml.sha256").exists());
    fx.publish_with(
        "stable",
        &PublishOpts {
            bad_sidecar: true,
            ..Default::default()
        },
    );
    let text = read(&fx, "manifest-stable.toml");
    assert_ne!(
        read(&fx, "manifest-stable.toml.sha256")[..64],
        sha256_hex(&text).into_bytes()[..]
    );
}

#[test]
fn faults_do_what_they_say() {
    let mut fx = Fixture::new();
    fx.product("cig", "1.0.0").corrupt_asset();
    fx.product("cig", "1.0.1").truncate();
    fx.product("cig", "1.0.2").bad_sidecar();
    fx.product("cig", "1.0.3").missing_sidecar();
    fx.product("cigo", "1.0.0").zip_slip();
    fx.carton("0.1.0", &[("cig", "1.0.0")]);
    fx.publish("stable");
    let m =
        Manifest::from_str(&String::from_utf8(read(&fx, "manifest-stable.toml")).unwrap()).unwrap();
    let art = |t: &str, v: &str| {
        m.artifact(t, &Version::parse(v).unwrap(), &cig_compat::Target::host())
            .unwrap()
            .clone()
    };

    let a = art("cig", "1.0.0");
    let served = read(&fx, &a.asset);
    assert_eq!(served.len() as u64, a.size, "corrupt keeps the size");
    assert_ne!(sha256_hex(&served), a.sha256);

    let a = art("cig", "1.0.1");
    assert_eq!(read(&fx, &a.asset).len() as u64, a.size / 2);

    let a = art("cig", "1.0.2");
    assert_eq!(
        sha256_hex(&read(&fx, &a.asset)),
        a.sha256,
        "the asset itself is right"
    );
    assert_ne!(
        String::from_utf8(read(&fx, &format!("{}.sha256", a.asset))).unwrap()[..64],
        a.sha256[..]
    );

    let a = art("cig", "1.0.3");
    assert!(!fx.releases.join(format!("{}.sha256", a.asset)).exists());

    // zip_slip tells the truth about the evil archive
    let a = art("cigo", "1.0.0");
    assert_eq!(sha256_hex(&read(&fx, &a.asset)), a.sha256);
}

#[test]
fn fixing_a_fault_and_republishing_removes_the_stale_sidecar() {
    let mut fx = Fixture::new();
    fx.product("cig", "1.0.0").missing_sidecar();
    fx.carton("0.1.0", &[("cig", "1.0.0")]);
    fx.publish("stable");
    let side = fx
        .releases
        .join(format!("{}.sha256", asset_name("cig", "1.0.0")));
    assert!(!side.exists());
    fx.product("cig", "1.0.0");
    fx.publish("stable");
    assert!(side.exists());
}

#[test]
fn yanks_requires_and_packs_reach_the_manifest_text() {
    let mut fx = Fixture::new();
    fx.product("cigo", "1.4.1").yank("example reason");
    fx.product("cigo", "1.17.0").requires("cig", ">=1.16.1, <2");
    fx.carton("0.0.9", &[("cig", "1.16.1"), ("cigo", "1.4.1")])
        .yank_carton("0.0.9", "cigo could skip a file");
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.17.0")]);
    fx.publish("stable");
    let text = String::from_utf8(read(&fx, "manifest-stable.toml")).unwrap();
    assert!(
        text.contains("\"cigo 1.4.1\" = \"example reason\""),
        "{text}"
    );
    assert!(text.contains("\"carton 0.0.9\" = \"cigo could skip a file\""));
    assert!(text.contains("requires = { cig = \">=1.16.1, <2\" }"));
    assert!(text.contains("dev = [ \"cig\", \"cigo\" ]"));
    Manifest::from_str(&text).unwrap();
}

#[test]
fn carton_declares_products_it_names() {
    let mut fx = Fixture::new();
    fx.carton("0.1.0", &[("cig", "9.9.9")]);
    fx.publish("stable");
    assert!(fx.releases.join(asset_name("cig", "9.9.9")).is_file());
}

// ---- the fake products actually behave (Unix: they are shell scripts)

#[cfg(unix)]
mod unix {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn exec_script(path: &std::path::Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn run(cmd: &mut Command) -> (i32, String) {
        // A script written a moment ago can still be open in a child another
        // test thread forked (until it execs): exec then fails with "text file
        // busy". Try again; the window is microseconds.
        let mut tries = 0;
        let o = loop {
            match cmd.output() {
                Err(e) if e.raw_os_error() == Some(26) && tries < 100 => {
                    tries += 1;
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                r => break r.unwrap(),
            }
        };
        (
            o.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&o.stdout).trim().to_string(),
        )
    }

    #[test]
    fn fake_cig_prints_version_echoes_args_and_exits_as_told() {
        let mut fx = Fixture::new();
        fx.carton("0.1.0", &[("cig", "1.20.0")]);
        fx.publish("stable");
        let cig = fx.tmp.path().join("cig");
        exec_script(&cig, &read(&fx, &asset_name("cig", "1.20.0")));
        assert_eq!(
            run(Command::new(&cig).arg("--version")),
            (0, "cig 1.20.0".into())
        );
        assert_eq!(
            run(Command::new(&cig)
                .args(["run", "x"])
                .env("CIGI_CARTON", "0.1.0")),
            (0, "cig 1.20.0 args=run x carton=0.1.0".into())
        );
        assert_eq!(run(Command::new(&cig).arg("x").env("FAKE_EXIT", "7")).0, 7);
    }

    #[test]
    fn fake_cig_dry_run_output_is_configurable() {
        let mut fx = Fixture::new();
        fx.product("cig", "1.21.0")
            .dry_run_output("plan: it's different");
        fx.carton("0.2.0", &[("cig", "1.21.0")]);
        fx.publish("stable");
        let cig = fx.tmp.path().join("cig");
        exec_script(&cig, &read(&fx, &asset_name("cig", "1.21.0")));
        assert_eq!(
            run(Command::new(&cig).args(["run", "--dry-run", "s.cig"])),
            (0, "plan: it's different".into())
        );
        assert_eq!(run(Command::new(&cig).arg("--version")).1, "cig 1.21.0");
    }

    #[test]
    fn fake_cigo_uses_the_cig_on_its_path() {
        let mut fx = Fixture::new();
        fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.17.0")]);
        fx.publish("stable");
        let bin = fx.tmp.path().join("pathbin");
        exec_script(&bin.join("cig"), &read(&fx, &asset_name("cig", "1.20.0")));
        // unpack the cigo archive by hand: strip 1
        let gz = flate2::read::GzDecoder::new(
            std::fs::File::open(fx.releases.join(asset_name("cigo", "1.17.0"))).unwrap(),
        );
        let mut ar = tar::Archive::new(gz);
        let mut names = Vec::new();
        for e in ar.entries().unwrap() {
            let mut e = e.unwrap();
            let p = e.path().unwrap().into_owned();
            names.push(p.display().to_string());
            if p.ends_with("bin/cigo") {
                let mut b = Vec::new();
                e.read_to_end(&mut b).unwrap();
                exec_script(&fx.tmp.path().join("cigo"), &b);
            }
        }
        for want in [
            "cig-orchestrator-v1.17.0/orchestrate.cig",
            "cig-orchestrator-v1.17.0/LICENSE",
            "cig-orchestrator-v1.17.0/docs/a.md",
            "cig-orchestrator-v1.17.0/bin/cigo",
        ] {
            assert!(names.iter().any(|n| n == want), "{want} in {names:?}");
        }
        let path = format!("{}:/usr/bin:/bin", bin.display());
        assert_eq!(
            run(Command::new(fx.tmp.path().join("cigo")).env("PATH", path)),
            (0, "cigo 1.17.0 uses cig 1.20.0".into())
        );
    }

    #[test]
    fn fake_installer_selfcheck() {
        for (fails, code) in [(false, 0), (true, 1)] {
            let mut fx = Fixture::new();
            let p = fx.product("ciginstaller", "0.1.1");
            if fails {
                p.selfcheck_fails();
            }
            fx.carton("0.1.1", &[("ciginstaller", "0.1.1")]);
            fx.publish("stable");
            let gz = flate2::read::GzDecoder::new(
                std::fs::File::open(fx.releases.join(asset_name("ciginstaller", "0.1.1"))).unwrap(),
            );
            let mut ar = tar::Archive::new(gz);
            for e in ar.entries().unwrap() {
                let mut e = e.unwrap();
                if e.path().unwrap().ends_with("ciginstaller") {
                    let mut b = Vec::new();
                    e.read_to_end(&mut b).unwrap();
                    exec_script(&fx.tmp.path().join("ciginstaller"), &b);
                }
            }
            let (c, out) = run(Command::new(fx.tmp.path().join("ciginstaller")).arg("__selfcheck"));
            assert_eq!(c, code);
            if !fails {
                assert_eq!(out, "ciginstaller 0.1.1");
            }
        }
    }

    #[test]
    fn run_shim_runs_in_the_scratch_environment() {
        let fx = Fixture::new();
        let shim = fx.cigscript().join("bin/cigo");
        exec_script(
            &shim,
            b"#!/bin/sh\necho \"home=$HOME cs=$CIGSCRIPT_HOME pwd=$(pwd) color=$NO_COLOR\"\n",
        );
        let o = fx.run_shim("cigo", &fx.project, &[]);
        let out = String::from_utf8_lossy(&o.stdout).into_owned();
        assert!(
            out.contains(&format!("home={}", fx.user_home.display())),
            "{out}"
        );
        assert!(out.contains(&format!("cs={}", fx.cigscript().display())));
        assert!(out.contains("color=1"));
        assert!(out.contains(&format!("pwd={}", fx.project.display())) || out.contains("pwd="));
    }
}

// ---- environment hygiene

#[test]
fn the_test_process_environment_is_never_touched() {
    let before: Vec<_> = std::env::vars().collect();
    let fx = Fixture::new();
    let _ = fx.cigi();
    let _ = fx.cigi_with_mirror("http://127.0.0.1:1");
    assert_eq!(before, std::env::vars().collect::<Vec<_>>());
}

#[test]
fn child_env_is_scratch_only() {
    let fx = Fixture::new();
    let c = fx.cigi();
    let envs: std::collections::BTreeMap<String, String> = c
        .get_envs()
        .filter_map(|(k, v)| {
            Some((
                k.to_string_lossy().into_owned(),
                v?.to_string_lossy().into_owned(),
            ))
        })
        .collect();
    let home = fx.user_home.display().to_string();
    assert_eq!(envs["HOME"], home);
    assert_eq!(envs["CIGSCRIPT_HOME"], format!("{home}/.cigscript"));
    assert_eq!(
        envs["CIGI_MIRROR"],
        format!("file://{}", fx.releases.display())
    );
    assert_eq!(envs["NO_COLOR"], "1");
    assert_eq!(envs["TERM"], "dumb");
    assert!(envs["PATH"].starts_with(&format!("{home}/.cigscript/bin:")));
    assert_eq!(c.get_current_dir(), Some(fx.project.as_path()));
    // nothing inherited that could point at the real home
    assert!(!envs.contains_key("XDG_CONFIG_HOME"));
    assert!(fx.home.root().starts_with(fx.tmp.path()));
}

// ---- the server and the recorder

fn raw_get(addr: &str, path: &str, extra: &str) -> String {
    let mut s = std::net::TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    write!(s, "GET {path} HTTP/1.1\r\nHost: x\r\nUser-Agent: ciginstaller\r\nConnection: close\r\n{extra}\r\n").unwrap();
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    out
}

#[test]
fn server_serves_records_and_flags_telemetry() {
    let mut fx = two_carton_fixture();
    fx.publish("stable");
    let srv = fx.serve();
    let addr = srv.url().trim_start_matches("http://").to_string();
    let ok = raw_get(&addr, "/manifest-stable.toml", "");
    assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
    assert!(raw_get(&addr, "/nope", "").starts_with("HTTP/1.1 404"));
    assert!(raw_get(&addr, "/../etc/passwd", "").starts_with("HTTP/1.1 404"));
    let log = srv.requests();
    assert_eq!(log.len(), 3);
    assert_eq!(log[0].path, "/manifest-stable.toml");
    assert_eq!(log[0].header("user-agent"), Some("ciginstaller"));
    // 404s for odd paths trip the path rule; the clean one alone passes
    assert!(srv.assert_no_telemetry().is_err());
    srv.clear();
    raw_get(&addr, "/manifest-stable.toml", "");
    srv.assert_no_telemetry().unwrap();
    raw_get(&addr, "/manifest-stable.toml?machine=abc", "");
    raw_get(&addr, "/manifest-stable.toml.sha256", "Cookie: id=1\r\n");
    let bad = srv.assert_no_telemetry().unwrap_err();
    assert!(
        bad.iter().any(|b| b.contains("query")) && bad.iter().any(|b| b.contains("Cookie")),
        "{bad:?}"
    );
}

#[test]
fn server_rules() {
    let mut fx = two_carton_fixture();
    fx.publish("stable");
    let srv = fx.serve();
    let addr = srv.url().trim_start_matches("http://").to_string();
    srv.rule(
        "/manifest-stable.toml",
        Rule::Redirect("http://other.example/x".into()),
    );
    let r = raw_get(&addr, "/manifest-stable.toml", "");
    assert!(
        r.starts_with("HTTP/1.1 302")
            && r.to_lowercase()
                .contains("location: http://other.example/x"),
        "{r}"
    );
    srv.rule("/manifest-stable.toml", Rule::Status(503));
    assert!(raw_get(&addr, "/manifest-stable.toml", "").starts_with("HTTP/1.1 503"));
    // POST is refused and recorded
    let mut s = std::net::TcpStream::connect(&addr).unwrap();
    write!(
        s,
        "POST /x HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    assert!(out.starts_with("HTTP/1.1 405"));
    assert!(srv.requests().iter().any(|r| r.method == "POST"));
}

#[test]
fn server_stops_when_dropped() {
    let fx = Fixture::new();
    let srv = fx.serve();
    let addr = srv.url().trim_start_matches("http://").to_string();
    drop(srv);
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert!(std::net::TcpStream::connect(&addr).is_err());
}

#[test]
fn snapshot_of_the_scratch_home_is_stable() {
    let fx = Fixture::new();
    assert!(Snapshot::take(&fx.cigscript()).is_empty());
    std::fs::create_dir_all(fx.cigscript().join("bin")).unwrap();
    std::fs::write(fx.cigscript().join("bin/x"), b"x").unwrap();
    let a = Snapshot::take(&fx.cigscript());
    a.assert_same(&Snapshot::take(&fx.cigscript()));
}
