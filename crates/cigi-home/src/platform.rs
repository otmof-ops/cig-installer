// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! The only place in this crate where `cfg(unix)` / `cfg(windows)` appear.

use std::io;
use std::path::Path;

/// `.exe` on Windows, empty elsewhere.
pub fn exe_suffix() -> &'static str {
    if cfg!(windows) {
        ".exe"
    } else {
        ""
    }
}

#[cfg(unix)]
mod imp {
    use std::fs::{self, File, OpenOptions};
    use std::io;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
    use std::path::Path;

    pub fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
    }

    pub fn mode_of(path: &Path) -> Option<u32> {
        fs::symlink_metadata(path)
            .ok()
            .map(|m| m.permissions().mode() & 0o7777)
    }

    pub fn ino(path: &Path) -> Option<u64> {
        fs::metadata(path).ok().map(|m| m.ino())
    }

    pub fn nlink(path: &Path) -> Option<u64> {
        fs::metadata(path).ok().map(|m| m.nlink())
    }

    pub fn sync_dir(path: &Path) -> io::Result<()> {
        File::open(path)?.sync_all()
    }

    pub fn is_exec(path: &Path) -> bool {
        mode_of(path).map(|m| m & 0o111 != 0).unwrap_or(false)
    }

    pub fn create_private_file(path: &Path) -> io::Result<File> {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
    }

    /// The directory is not writable by group/others and is owned by the
    /// current user (root excepted). The current uid is learned by creating a
    /// probe file, which keeps this free of unsafe code and libc.
    pub fn dir_ok(path: &Path) -> Result<(), String> {
        let md = fs::symlink_metadata(path).map_err(|e| format!("cannot stat: {e}"))?;
        if md.file_type().is_symlink() {
            return Err("a symbolic link".to_string());
        }
        if !md.is_dir() {
            return Err("not a directory".to_string());
        }
        if md.permissions().mode() & 0o022 != 0 {
            return Err("writable by group or others".to_string());
        }
        let probe = path.join(format!(".cigi-probe-{}", crate::util::unique_suffix()));
        let f = create_private_file(&probe).map_err(|e| format!("not writable: {e}"))?;
        let mine = f.metadata().map(|m| m.uid());
        drop(f);
        let _ = fs::remove_file(&probe);
        let mine = mine.map_err(|e| format!("cannot stat probe: {e}"))?;
        if md.uid() != mine && md.uid() != 0 {
            return Err("owned by another user".to_string());
        }
        Ok(())
    }
}

#[cfg(not(unix))]
mod imp {
    use std::fs::{self, File, OpenOptions};
    use std::io;
    use std::path::Path;

    pub fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
        let mut p = fs::metadata(path)?.permissions();
        p.set_readonly(mode & 0o222 == 0);
        fs::set_permissions(path, p)
    }

    pub fn mode_of(_path: &Path) -> Option<u32> {
        None
    }

    pub fn ino(_path: &Path) -> Option<u64> {
        None
    }

    pub fn nlink(_path: &Path) -> Option<u64> {
        None
    }

    pub fn sync_dir(_path: &Path) -> io::Result<()> {
        Ok(())
    }

    pub fn is_exec(path: &Path) -> bool {
        path.extension()
            .map(|e| {
                let e = e.to_string_lossy().to_ascii_lowercase();
                e == "exe" || e == "cmd" || e == "bat" || e == "ps1"
            })
            .unwrap_or(false)
    }

    pub fn create_private_file(path: &Path) -> io::Result<File> {
        OpenOptions::new().write(true).create_new(true).open(path)
    }

    pub fn dir_ok(path: &Path) -> Result<(), String> {
        let md = fs::symlink_metadata(path).map_err(|e| format!("cannot stat: {e}"))?;
        if md.file_type().is_symlink() {
            return Err("a symbolic link".to_string());
        }
        if md.is_dir() {
            Ok(())
        } else {
            Err("not a directory".to_string())
        }
    }
}

/// Set permission bits (Windows: only the read-only attribute follows).
pub fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    imp::set_mode(path, mode)
}

/// Permission bits (`0o7777` mask) or `None` where the platform has none.
pub fn mode_of(path: &Path) -> Option<u32> {
    imp::mode_of(path)
}

/// Inode number, where the platform has one. Equal inodes mean one file.
pub fn ino(path: &Path) -> Option<u64> {
    imp::ino(path)
}

/// Hard link count, where the platform reports one.
pub fn nlink(path: &Path) -> Option<u64> {
    imp::nlink(path)
}

/// fsync a directory so a rename inside it is durable (no-op on Windows).
pub fn sync_dir(path: &Path) -> io::Result<()> {
    imp::sync_dir(path)
}

/// Whether the file has an executable bit (Windows: by extension).
pub fn is_exec(path: &Path) -> bool {
    imp::is_exec(path)
}

/// Create a new file, owner-only where the platform has modes.
pub fn create_private_file(path: &Path) -> io::Result<std::fs::File> {
    imp::create_private_file(path)
}

/// Section 4.1: refuse a directory that others can write to or that another
/// user owns (root excepted). The error is the reason.
pub fn dir_ok(path: &Path) -> Result<(), String> {
    imp::dir_ok(path)
}
