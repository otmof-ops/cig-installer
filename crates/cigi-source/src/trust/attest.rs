// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `gh attestation verify` behind a trait, so tests need no `gh` and no network.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub trait Attest: Send + Sync {
    /// Can this verifier run at all (is `gh` on PATH)?
    fn available(&self) -> bool;
    /// Verify `file` against `repo`'s attestations (`bundle`: an attestation file for offline use).
    /// `Ok(who)` names what vouched; `Err(why)` is a short reason.
    fn verify(&self, file: &Path, repo: &str, bundle: Option<&Path>) -> Result<String, String>;
}

/// No attestation checks (the default in tests and when `gh` is absent by choice).
#[derive(Debug, Clone, Copy, Default)]
pub struct NoAttest;
impl Attest for NoAttest {
    fn available(&self) -> bool {
        false
    }
    fn verify(&self, _file: &Path, _repo: &str, _bundle: Option<&Path>) -> Result<String, String> {
        Err("attestation checking is off".into())
    }
}

/// A fixed answer, for tests.
#[derive(Debug, Clone)]
pub struct StubAttest(pub Result<String, String>);
impl Attest for StubAttest {
    fn available(&self) -> bool {
        true
    }
    fn verify(&self, _file: &Path, _repo: &str, _bundle: Option<&Path>) -> Result<String, String> {
        self.0.clone()
    }
}

/// A state and cache folder of `gh`'s own for one run of it, removed when this is dropped.
///
/// `gh` keeps state (a device id, update checks) and a cache under the user's XDG folders. The
/// installer's `gh` calls point those at a fresh private folder, so a run leaves nothing in the
/// user's state or cache folder. `gh`'s configuration and login (`GH_CONFIG_DIR`, the config
/// folder) are left as they are.
pub struct GhScratch(PrivateDir);

impl GhScratch {
    /// A fresh 0700 folder in the system temp directory.
    pub fn new() -> std::io::Result<GhScratch> {
        PrivateDir::new(None).map(GhScratch)
    }

    /// Point `cmd`'s state and cache folders at this one.
    pub fn apply(&self, cmd: &mut Command) {
        cmd.env("XDG_STATE_HOME", self.0.path().join("state"))
            .env("XDG_CACHE_HOME", self.0.path().join("cache"));
    }
}

/// Runs `gh attestation verify <file> -R <repo> --owner otmof-ops` with output discarded.
#[derive(Debug, Clone, Default)]
pub struct GhAttest;

fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

impl GhAttest {
    /// Is `gh` on PATH? (Same as `available`; a plain method so callers need not import the trait.)
    pub fn available_cached(&self) -> bool {
        find_on_path("gh").is_some() || find_on_path("gh.exe").is_some()
    }
}

impl Attest for GhAttest {
    fn available(&self) -> bool {
        find_on_path("gh").is_some() || find_on_path("gh.exe").is_some()
    }
    fn verify(&self, file: &Path, repo: &str, bundle: Option<&Path>) -> Result<String, String> {
        let scratch = GhScratch::new().map_err(|e| format!("could not make gh's folder: {e}"))?;
        let mut cmd = Command::new("gh");
        scratch.apply(&mut cmd);
        cmd.arg("attestation")
            .arg("verify")
            .arg(file)
            .arg("-R")
            .arg(repo)
            .arg("--owner")
            .arg(crate::pins::ATTEST_OWNER);
        if let Some(b) = bundle {
            cmd.arg("--bundle").arg(b);
        }
        let mut child = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("could not run gh: {e}"))?;
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            match child.try_wait() {
                Ok(Some(st)) if st.success() => return Ok(format!("github:{repo}")),
                Ok(Some(st)) => {
                    return Err(format!(
                        "gh attestation verify exited with {}",
                        st.code().unwrap_or(-1)
                    ))
                }
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(50))
                }
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("gh attestation verify took longer than 60 seconds".into());
                }
                Err(e) => return Err(format!("could not wait for gh: {e}")),
            }
        }
    }
}

/// A fresh 0700 directory (under `installer/cache` when given, else the system temp directory)
/// that holds the files handed to `gh`, removed with everything in it on drop.
pub(crate) struct PrivateDir(PathBuf);
impl PrivateDir {
    pub fn new(under: Option<&Path>) -> std::io::Result<PrivateDir> {
        let base = under
            .map(Path::to_path_buf)
            .unwrap_or_else(std::env::temp_dir);
        let p = base.join(format!("attest-{}", crate::digest::random_suffix()));
        #[cfg_attr(not(unix), allow(unused_mut))] // `mode` is Unix only
        let mut b = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            b.mode(0o700);
        }
        b.create(&p)?;
        Ok(PrivateDir(p))
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
    pub fn put(&self, name: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
        let p = self.0.join(name);
        let mut f = crate::http::create_private(&p)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        Ok(p)
    }
}
impl Drop for PrivateDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::GhScratch;
    use std::process::Command;

    #[test]
    fn gh_gets_a_state_and_cache_folder_of_its_own_that_goes_when_it_is_done() {
        let s = GhScratch::new().unwrap();
        let mut cmd = Command::new("gh");
        s.apply(&mut cmd);
        let envs: Vec<(String, String)> = cmd
            .get_envs()
            .filter_map(|(k, v)| {
                Some((
                    k.to_string_lossy().into_owned(),
                    v?.to_string_lossy().into_owned(),
                ))
            })
            .collect();
        let dir = s.0.path().to_path_buf();
        assert!(dir.is_dir());
        for (k, sub) in [("XDG_STATE_HOME", "state"), ("XDG_CACHE_HOME", "cache")] {
            let v = envs.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
            assert_eq!(v, Some(dir.join(sub).to_string_lossy().into_owned()), "{k}");
        }
        // gh's login and settings are left where they are
        assert!(envs
            .iter()
            .all(|(k, _)| k != "XDG_CONFIG_HOME" && k != "GH_CONFIG_DIR"));
        drop(s);
        assert!(!dir.exists());
    }
}
