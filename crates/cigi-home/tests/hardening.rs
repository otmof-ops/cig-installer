// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! Regression tests from the security review of PR #36: shim-index paths and home directories.

use cigi_home::state::Stick;
use cigi_home::*;
use std::fs;

fn receipt_with_shim(h: &Home, shim_path: &str) {
    let v = cig_compat::Version::parse("1.0.0").unwrap();
    let dir = h.toolchain("cig", &v);
    fs::create_dir_all(&dir).unwrap();
    let r = serde_json::json!({"format":1,"tool":"cig","version":"1.0.0","installed_at":"x","from_carton":null,"txn":"t",
      "source":{"repo":"a/b","tag":"t","asset":"a","origin":"o"},"artifact":{"sha256":"0","size":0},
      "trust":{"level":"checksummed","independent":true,"attested_by":null,"manifest_sequence":1},
      "files":[],"shims":[{"name":"cig","path":shim_path}],"humidor_mirror":null});
    fs::write(dir.join(".cigi-receipt.json"), r.to_string()).unwrap();
}

fn stick_state() -> State {
    let mut st = State::default();
    st.sticks.push(Stick {
        tool: "cig".into(),
        version: "1.0.0".into(),
        from_carton: "0.1.0".into(),
        installed_at: "x".into(),
        reason: "x".into(),
    });
    st
}

#[test]
fn shim_index_never_leaves_the_toolchain() {
    for bad in [
        "../../../../../../../bin/sh",
        "bin/../../x",
        "/bin/sh",
        "",
        "bin\\x",
    ] {
        let t = tempfile::tempdir().unwrap();
        let h = Home::at(t.path().join(".cigscript"));
        h.ensure_layout().unwrap();
        receipt_with_shim(&h, bad);
        let idx = ShimIndex::from_state(&h, &stick_state());
        assert!(
            idx.sticks.is_empty(),
            "{bad:?} reached the index: {}",
            idx.render()
        );
    }
    // the plain path still works
    let t = tempfile::tempdir().unwrap();
    let h = Home::at(t.path().join(".cigscript"));
    h.ensure_layout().unwrap();
    receipt_with_shim(&h, "bin/cig");
    let idx = ShimIndex::from_state(&h, &stick_state());
    assert!(idx.render().contains("toolchains/cig/1.0.0/bin/cig"));
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn ensure_layout_refuses_a_symlinked_directory() {
        let t = tempfile::tempdir().unwrap();
        let elsewhere = t.path().join("elsewhere");
        fs::create_dir(&elsewhere).unwrap();
        let root = t.path().join(".cigscript");
        fs::create_dir(&root).unwrap();
        symlink(&elsewhere, root.join("installer")).unwrap();
        let e = Home::at(root).ensure_layout().unwrap_err();
        assert_eq!(e.code(), "E1835", "{e}");
        assert!(elsewhere.read_dir().unwrap().next().is_none());
    }

    #[test]
    fn check_permissions_covers_every_writing_directory_and_refuses_links() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let h = Home::at(t.path().join(".cigscript"));
        h.ensure_layout().unwrap();
        h.check_permissions().unwrap();
        for sub in [
            h.installer().join("cache").join("downloads"),
            h.store().join("tmp"),
            h.txn_root(),
        ] {
            fs::set_permissions(&sub, fs::Permissions::from_mode(0o777)).unwrap();
            assert_eq!(
                h.check_permissions().unwrap_err().code(),
                "E1835",
                "{sub:?}"
            );
            fs::set_permissions(&sub, fs::Permissions::from_mode(0o700)).unwrap();
        }
        // a toolchains/<tool> directory
        let tool = h.toolchains().join("cig");
        fs::create_dir(&tool).unwrap();
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o775)).unwrap();
        assert_eq!(h.check_permissions().unwrap_err().code(), "E1835");
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
        h.check_permissions().unwrap();
        // a symlink in place of a directory
        let down = h.installer().join("cache").join("downloads");
        fs::remove_dir(&down).unwrap();
        let other = t.path().join("other");
        fs::create_dir(&other).unwrap();
        symlink(&other, &down).unwrap();
        let e = h.check_permissions().unwrap_err();
        assert_eq!(e.code(), "E1835");
        assert!(e.to_string().contains("symbolic link"), "{e}");
    }

    #[test]
    fn a_symlinked_root_is_allowed_when_its_target_is_safe() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let real = t.path().join("real");
        fs::create_dir(&real).unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o700)).unwrap();
        let root = t.path().join(".cigscript");
        symlink(&real, &root).unwrap();
        let h = Home::at(root);
        h.ensure_layout().unwrap();
        h.check_permissions().unwrap();
        assert!(real.join("installer").is_dir());
    }
}
