// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `cargo xtask fixture-mirror <dir>`: the testkit's fixture mirror, written outside cargo tests.
//!
//! The mirror is what the acceptance tests install from: a flat directory with
//! `manifest-stable.toml`, its `.sha256`, and every product's build with a real sidecar. The
//! products are fakes (a `cig` shell script, a `cigo` tarball), built for the machine that runs
//! this command, so a fresh-install job can prove the real binary on distributions that have no
//! Rust toolchain. It is one source, which the installer refuses by default; the job that uses
//! it opts in with `CIGI_ALLOW_SINGLE_SOURCE=true`, as the tests do.
//!
//! With `--candidate <ver>` (and optional `--candidate-cig <ver>`, `--candidate-cigo <ver>`),
//! the mirror also publishes a second carton alongside the base one, for the upgrade matrix
//! (`ci/upgrade.sh`). The candidate shares the base's members unless a tool is named, so by
//! default the two cartons differ only in the carton version itself and point at the same
//! product builds; an override gives the candidate a different build of that tool.

use std::path::Path;

use cigi_testkit::Fixture;

/// The base carton the mirror holds.
pub const CARTON: &str = "0.1.0";
/// The members of the base carton: `(tool, version)`.
pub const MEMBERS: [(&str, &str); 2] = [("cig", "1.20.0"), ("cigo", "1.16.0")];

/// A carton to add to the mirror in addition to the base.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub version: String,
    pub cig: String,
    pub cigo: String,
}

/// Write the mirror into `dir` (made when missing; files already there are overwritten). Returns
/// how many files were written.
pub fn write(dir: &Path) -> Result<usize, String> {
    write_with(dir, None)
}

/// As `write`, with a second carton added for upgrade tests.
pub fn write_with(dir: &Path, candidate: Option<&Candidate>) -> Result<usize, String> {
    let mut fx = Fixture::new();
    fx.carton(CARTON, &MEMBERS);
    if let Some(c) = candidate {
        fx.carton(
            &c.version,
            &[("cig", c.cig.as_str()), ("cigo", c.cigo.as_str())],
        );
    }
    fx.publish("stable");
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut n = 0;
    let entries = std::fs::read_dir(&fx.releases).map_err(|e| format!("read the mirror: {e}"))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("read the mirror: {e}"))?;
        let to = dir.join(entry.file_name());
        std::fs::copy(entry.path(), &to).map_err(|e| format!("{}: {e}", to.display()))?;
        n += 1;
    }
    if n == 0 {
        return Err("the fixture published nothing".into());
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mirror_holds_a_manifest_with_its_sidecar_and_the_builds() {
        let t = std::env::temp_dir().join(format!("cigi-xtask-mirror-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        let n = write(&t).unwrap();
        assert!(n >= 6, "{n} files");
        assert!(t.join("manifest-stable.toml").is_file());
        assert!(t.join("manifest-stable.toml.sha256").is_file());
        let names: Vec<String> = std::fs::read_dir(&t)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            names.iter().any(|n| n.starts_with("cig-v1.20.0")),
            "{names:?}"
        );
        assert!(
            names
                .iter()
                .any(|n| n.starts_with("cig-orchestrator-v1.16.0")),
            "{names:?}"
        );
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn a_candidate_carton_shares_the_mirror_with_the_base() {
        let t = std::env::temp_dir().join(format!("cigi-xtask-mirror-cand-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        let candidate = Candidate {
            version: "0.2.0-fixture.1".into(),
            cig: "1.22.0-fixture.1".into(),
            cigo: "1.17.0-fixture.1".into(),
        };
        write_with(&t, Some(&candidate)).unwrap();
        let manifest = std::fs::read_to_string(t.join("manifest-stable.toml")).unwrap();
        assert!(manifest.contains("[carton.\"0.1.0\"]"), "{manifest}");
        assert!(
            manifest.contains("[carton.\"0.2.0-fixture.1\"]"),
            "{manifest}"
        );
        let names: Vec<String> = std::fs::read_dir(&t)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            names.iter().any(|n| n.starts_with("cig-v1.22.0-fixture.1")),
            "{names:?}"
        );
        assert!(
            names
                .iter()
                .any(|n| n.starts_with("cig-orchestrator-v1.17.0-fixture.1")),
            "{names:?}"
        );
        let _ = std::fs::remove_dir_all(&t);
    }
}
