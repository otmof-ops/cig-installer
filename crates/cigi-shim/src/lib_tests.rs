// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

use super::pins::{cig_from_json, header_pin, pin_from_toml};
use super::script_arg;
use std::ffi::OsString;

#[test]
fn toml_pin_ignores_arrays_and_strings_that_look_like_tables() {
    let t = "m = [\n  [1, 2],\n  [3, 4],\n]\ncarton = \"0.3\" # pin\n[deps]\n";
    assert_eq!(pin_from_toml(t), Some("0.3".into()));
    let s = "doc = \"\"\"\n[not a table]\ncarton = \"9\"\n\"\"\"\ncarton = '0.4'\n";
    assert_eq!(pin_from_toml(s), Some("0.4".into()));
    let l = "doc = '''\n[x]\n'''\ncarton = \"1\"\n";
    assert_eq!(pin_from_toml(l), Some("1".into()));
    // brackets in comments and strings do not move the depth
    let c = "a = \"[\" # [\nb = 1 # ]\ncarton = \"2\"\n";
    assert_eq!(pin_from_toml(c), Some("2".into()));
    // an inline table spread over lines
    let i = "t = {\n x = [1],\n}\ncarton = \"5\"\n";
    assert_eq!(pin_from_toml(i), Some("5".into()));
    // a real table ends the top level
    assert_eq!(pin_from_toml("a = 1\n[t]\ncarton = \"6\"\n"), None);
    // a key inside an open array is not the key
    assert_eq!(pin_from_toml("m = [\ncarton = \"7\",\n]\n"), None);
}

/// The gap between the checks and the open (the security re-review of PR #37): even a fifo reached by
/// the open itself returns at once, and the check on the opened file then refuses it.
#[cfg(target_os = "linux")]
#[test]
fn opening_a_fifo_never_waits_for_a_writer() {
    let dir = std::env::temp_dir().join(format!("cigi-shim-fifo-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let fifo = dir.join(".cig-carton");
    let made = std::process::Command::new("mkfifo").arg(&fifo).status();
    if !made.is_ok_and(|s| s.success()) {
        return; // no mkfifo here: nothing to prove
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let p = fifo.clone();
    std::thread::spawn(move || {
        let r = super::open_nonblocking(&p).map(|f| f.metadata().map(|m| m.is_file()));
        let _ = tx.send(r.is_ok());
    });
    let opened = rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("the open of a fifo waited for a writer");
    assert!(opened);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_header_pin_is_found_the_way_cig_finds_it() {
    let pin = |s: &str| header_pin(s).map(|p| (p.text, p.line));
    assert_eq!(
        pin("# cig >=1.6.2\nroll x = 1\n"),
        Some((">=1.6.2".into(), 1))
    );
    assert_eq!(
        pin("#!/usr/bin/env cig\n# cig 1.1\n"),
        Some(("1.1".into(), 2))
    );
    assert_eq!(pin("\n\n#cig: v1.1.2\n"), Some(("v1.1.2".into(), 3)));
    assert_eq!(pin("\u{feff}# cig 1\n"), Some(("1".into(), 1)));
    // other comments before it are fine; the first statement ends the header
    assert_eq!(pin("# a tool\n# cig 1.2\n"), Some(("1.2".into(), 2)));
    assert_eq!(pin("roll x = 1\n# cig 1.1\n"), None);
    // usage lines and directives are not pins; a shebang after the first line is an ordinary
    // comment, which cig reads past
    assert_eq!(pin("# cig run s.cig -- x\nroll x = 1\n"), None);
    assert_eq!(pin("# cig: run --dry-run\n"), None);
    assert_eq!(
        pin("
#!/usr/bin/env cig
# cig 1.1
"),
        Some(("1.1".into(), 3))
    );
    assert_eq!(pin(""), None);
}

#[test]
fn the_script_is_the_first_argument_that_is_not_a_flag() {
    let a = |v: &[&str]| v.iter().map(OsString::from).collect::<Vec<_>>();
    let name = |v: &[&str]| {
        let args = a(v);
        script_arg(&args).map(|s| s.to_string_lossy().into_owned())
    };
    assert_eq!(name(&["s.cig"]), Some("s.cig".into()));
    assert_eq!(name(&["run", "s.cig", "--", "x"]), Some("s.cig".into()));
    assert_eq!(
        name(&["--no-check", "check", "--deny-warnings", "s.cig"]),
        Some("s.cig".into())
    );
    assert_eq!(name(&["run", "--", "-odd.cig"]), Some("-odd.cig".into()));
    assert_eq!(name(&["--version"]), None);
    assert_eq!(name(&[]), None);
    assert_eq!(name(&["run"]), None);
}

#[test]
fn the_cig_key_is_the_top_level_one() {
    assert_eq!(
        cig_from_json(r#"{"project":"p","cig":">=1.20.0","components":{"cig":{"path":"."}}}"#),
        Some(">=1.20.0".into())
    );
    assert_eq!(cig_from_json(r#"{"components":{"cig":"1.2"}}"#), None);
    assert_eq!(cig_from_json(r#"{"cig":{"path":"."}}"#), None);
}

// Unix only (symlinks): a home named through a link is still the home of the real path under it
// (macOS: HOME is /var/..., a cwd is /private/var/...), and the walk stops there.
#[cfg(unix)]
#[test]
fn a_home_named_through_a_link_bounds_the_walk_of_its_real_path() {
    let tmp = std::env::temp_dir().join(format!("cigi-bound-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    // the temp directory is itself behind a link on macOS: every expected path starts canonical
    let base = std::fs::canonicalize(&tmp).unwrap();
    let real = base.join("private/home");
    std::fs::create_dir_all(real.join("a/b")).unwrap();
    std::os::unix::fs::symlink(base.join("private"), base.join("link")).unwrap();
    let home_by_link = base.join("link/home");
    let cwd = real.join("a/b"); // what getcwd reports: the real path
    let canon_home = std::fs::canonicalize(&home_by_link).unwrap();
    let (start, stop) = super::bound(&cwd, Some(&home_by_link));
    assert_eq!(start.as_ref(), cwd.as_path());
    assert_eq!(stop.as_deref(), Some(canon_home.as_path()));
    // the other way round: the home real, the start through the link
    let via_link = base.join("link/home/a/b");
    let (start, stop) = super::bound(&via_link, Some(&canon_home));
    assert_eq!(start.as_ref(), cwd.as_path());
    assert_eq!(stop.as_deref(), Some(canon_home.as_path()));
    // outside the home altogether: no stop
    let (_, stop) = super::bound(&base, Some(&home_by_link));
    assert!(stop.is_none());
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn shim_table_maps_every_shim_to_a_tool_in_tools() {
    // every shim's tool is one TOOLS knows, and every tool in TOOLS has a shim of its own name
    for (shim, tool) in super::SHIMS {
        assert!(
            super::TOOLS.contains(&tool),
            "{shim} serves {tool}, which is not a tool"
        );
    }
    for t in super::TOOLS {
        assert_eq!(super::tool_of(t), Some(t), "no shim for the tool {t}");
    }
    // the second shim of a tool: cigar's language server runs the carton's cigar
    assert_eq!(super::tool_of("cigar-lsp"), Some("cigar"));
    let cigar_shims = super::shims_of("cigar");
    assert!(cigar_shims.contains(&"cigar") && cigar_shims.contains(&"cigar-lsp"));
    assert_eq!(super::shims_of("cig"), vec!["cig"]);
    // a name no shim serves: the shim refuses it (E1844), the installer leaves it out
    assert_eq!(super::tool_of("cig-lsp"), None);
    assert_eq!(super::tool_of("evil"), None);
    assert!(super::shims_of("ciginstaller").is_empty());
}

/// A home folder as the installer makes it: `installer/` in it.
fn a_home(base: &std::path::Path, name: &str) -> std::path::PathBuf {
    let h = base.join(name);
    std::fs::create_dir_all(h.join("installer")).unwrap();
    std::fs::create_dir_all(h.join("bin")).unwrap();
    h
}

#[test]
fn a_tool_belongs_to_the_home_it_lives_in() {
    let t = std::env::temp_dir().join(format!("cigi-shim-own-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&t);
    let home = a_home(&t, "home");
    // a shim in bin/, a build under toolchains/
    assert_eq!(
        super::home_containing(&home.join("bin/cig")),
        Some(home.clone())
    );
    let tc = home.join("toolchains/cig/1.21.0/bin");
    std::fs::create_dir_all(&tc).unwrap();
    assert_eq!(super::home_containing(&tc.join("cig")), Some(home.clone()));
    // a bin/ that is not a home's, and a development build: no home of their own
    std::fs::create_dir_all(t.join("loose/bin")).unwrap();
    assert_eq!(super::home_containing(&t.join("loose/bin/cig")), None);
    assert_eq!(
        super::home_containing(&t.join("target/release/ciginstaller")),
        None
    );
    let _ = std::fs::remove_dir_all(&t);
}

#[test]
fn the_home_a_shim_lives_in_wins_over_cigscript_home() {
    let t = std::env::temp_dir().join(format!("cigi-shim-choose-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&t);
    let home = a_home(&t, "home");
    let inbox = t.join("inbox/run/home");
    let shim = home.join("bin/cig");
    // CIGSCRIPT_HOME names another folder (cig's box sets it to the run's inbox): the shim's own
    // home is used, and the disagreement is still visible
    let c = super::choose_home(Some(&shim), Some(inbox.clone()), Some(t.clone())).unwrap();
    assert_eq!(c.home, home);
    assert!(c.owned);
    assert_eq!(c.env.as_deref(), Some(inbox.as_path()));
    assert!(c.disagreement().is_some());
    // a binary in no home follows CIGSCRIPT_HOME, then ~/.cigscript
    let dev = t.join("target/debug/cig");
    let c = super::choose_home(Some(&dev), Some(inbox.clone()), Some(t.clone())).unwrap();
    assert_eq!((c.home.clone(), c.owned), (inbox.clone(), false));
    let c = super::choose_home(Some(&dev), None, Some(t.clone())).unwrap();
    assert_eq!(c.home, t.join(".cigscript"));
    // a bin/ that is not a home yet: CIGSCRIPT_HOME first, else that folder, as before
    let loose = t.join("loose/bin/cig");
    let c = super::choose_home(Some(&loose), Some(inbox.clone()), None).unwrap();
    assert_eq!(c.home, inbox);
    let c = super::choose_home(Some(&loose), None, None).unwrap();
    assert_eq!(c.home, t.join("loose"));
    let _ = std::fs::remove_dir_all(&t);
}
