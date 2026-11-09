// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0
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

/// Runs `gh attestation verify <file> -R <repo>` with output discarded.
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
        let mut cmd = Command::new("gh");
        cmd.arg("attestation")
            .arg("verify")
            .arg(file)
            .arg("-R")
            .arg(repo);
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

/// A private temp file holding `bytes`, removed on drop. Used to hand a manifest to `gh`.
pub(crate) struct TempBytes(pub PathBuf);
impl TempBytes {
    pub fn new(tag: &str, bytes: &[u8]) -> std::io::Result<TempBytes> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "cigi-{}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed),
            tag
        ));
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&p)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        Ok(TempBytes(p))
    }
}
impl Drop for TempBytes {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
