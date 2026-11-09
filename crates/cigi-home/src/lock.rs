// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0

//! The installer lock: exclusive, advisory, `installer/lock`.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::thread::sleep;
use std::time::{Duration, Instant};

use fs4::fs_std::FileExt;

use crate::error::LockError;
use crate::home::Home;

/// How long `Home::lock` waits for another installer.
pub const LOCK_WAIT: Duration = Duration::from_secs(30);

/// Held for the life of the value; released when dropped (or the process dies).
#[derive(Debug)]
pub struct InstallerLock {
    // Kept open: closing the file releases the lock.
    _file: File,
    path: PathBuf,
    root: PathBuf,
}

impl InstallerLock {
    /// The lock file's path.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    pub(crate) fn belongs_to(&self, home: &Home) -> bool {
        self.root == home.root()
    }
}

impl Home {
    /// Exclusive lock, waiting up to 30 s for another installer (then `E1834`).
    pub fn lock(&self) -> Result<InstallerLock, LockError> {
        self.lock_with(LOCK_WAIT, &mut |_| {})
    }

    /// As [`Home::lock`] with a wait budget and a callback, called once with
    /// the holder's pid (when known) at the first contention: the caller
    /// prints "waiting for another ciginstaller (pid N)".
    pub fn lock_with(
        &self,
        wait: Duration,
        on_wait: &mut dyn FnMut(Option<u32>),
    ) -> Result<InstallerLock, LockError> {
        let path = self.lock_path();
        let io_err = |source: io::Error| LockError::Io {
            path: path.clone(),
            source,
        };
        fs::create_dir_all(self.installer()).map_err(io_err)?;
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(io_err)?;
        let deadline = Instant::now() + wait;
        let mut announced = false;
        loop {
            match FileExt::try_lock_exclusive(&file) {
                Ok(true) => break,
                Ok(false) => {}
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(io_err(e)),
            }
            let pid = fs::read_to_string(&path)
                .ok()
                .and_then(|s| s.trim().parse::<u32>().ok());
            if !announced {
                announced = true;
                on_wait(pid);
            }
            if Instant::now() >= deadline {
                return Err(LockError::Held { path, pid });
            }
            sleep(Duration::from_millis(25));
        }
        file.set_len(0).map_err(io_err)?;
        writeln!(file, "{}", std::process::id()).map_err(io_err)?;
        file.flush().map_err(io_err)?;
        Ok(InstallerLock {
            _file: file,
            path,
            root: self.root(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_lock_in_process_waits_then_e1834() {
        let t = tempfile::tempdir().unwrap();
        let h = Home::at(t.path().join(".cigscript"));
        let first = h.lock().unwrap();
        let mut seen = None;
        let started = Instant::now();
        let err = h
            .lock_with(Duration::from_millis(200), &mut |pid| seen = Some(pid))
            .unwrap_err();
        assert_eq!(err.code(), "E1834");
        assert!(started.elapsed() >= Duration::from_millis(200));
        assert_eq!(seen, Some(Some(std::process::id())));
        drop(first);
        h.lock().unwrap();
    }
}
