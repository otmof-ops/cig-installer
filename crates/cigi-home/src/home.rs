// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! `Home`: where everything lives, and the layout it must have (sections 4, 4.1).

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use cig_compat::Version;

use crate::error::HomeError;
use crate::platform;
use crate::store::Store;

/// The file in `installer/` that says this installer made the home.
pub const OWNER_FILE: &str = ".owner";
/// What it says; `uninstall --all` accepts a marker that starts with the first word.
pub const OWNER_TEXT: &str = "ciginstaller home, format 1\n";

/// The `~/.cigscript` directory (or `CIGSCRIPT_HOME`) and the user's own home,
/// which profiles live under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Home {
    root: PathBuf,
    user_home: PathBuf,
}

impl Home {
    /// `$CIGSCRIPT_HOME`, else `$HOME/.cigscript`, else `$USERPROFILE\.cigscript`.
    pub fn discover() -> Result<Home, HomeError> {
        Home::discover_with(&|k| std::env::var_os(k))
    }

    /// As [`Home::discover`] with an explicit environment (tests).
    pub fn discover_with(env: &dyn Fn(&str) -> Option<OsString>) -> Result<Home, HomeError> {
        let nonempty = |k: &str| env(k).filter(|v| !v.is_empty()).map(PathBuf::from);
        let user = nonempty("HOME").or_else(|| nonempty("USERPROFILE"));
        let root = match nonempty("CIGSCRIPT_HOME") {
            Some(r) => r,
            None => match &user {
                Some(u) => u.join(".cigscript"),
                None => return Err(HomeError::NoHome),
            },
        };
        let user_home = user.unwrap_or_else(|| default_user_home(&root));
        Ok(Home { root, user_home })
    }

    /// A home rooted at `root` (tests). The user's home is `root`'s parent.
    pub fn at(root: PathBuf) -> Home {
        let user_home = default_user_home(&root);
        Home { root, user_home }
    }

    /// Same home, with the user's own home directory set (profile files).
    pub fn with_user_home(mut self, user_home: PathBuf) -> Home {
        self.user_home = user_home;
        self
    }

    pub fn root(&self) -> PathBuf {
        self.root.clone()
    }

    /// The user's home directory (where `.profile` lives).
    pub fn user_home(&self) -> PathBuf {
        self.user_home.clone()
    }

    pub fn bin(&self) -> PathBuf {
        self.root.join("bin")
    }

    pub fn toolchains(&self) -> PathBuf {
        self.root.join("toolchains")
    }

    /// The store of this home: `humidor/store/`, where the humidor is the shared shelf. A home
    /// made before the move still has `store/` and no `humidor/store/` until the first command
    /// that writes migrates it ([`crate::migrate`]); until then this answers the old place, so a
    /// command that only reads sees the objects wherever they are.
    pub fn store(&self) -> PathBuf {
        if self.is_legacy_store() {
            self.legacy_store()
        } else {
            self.shared_store()
        }
    }

    /// `humidor/store/`: where the store lives from this layout on.
    pub fn shared_store(&self) -> PathBuf {
        self.humidor().join("store")
    }

    /// `store/`: where the objects were before the humidor became the shared store.
    pub fn legacy_store(&self) -> PathBuf {
        self.root.join("store")
    }

    /// The home still has the old `store/objects` and no `humidor/store/objects`. Asked of the
    /// objects, not of the directories, so a migration cut short after it made `humidor/store/`
    /// and before it moved anything still reads the old place until `recover` has undone it.
    pub fn is_legacy_store(&self) -> bool {
        fs::symlink_metadata(self.shared_store().join("objects")).is_err()
            && fs::symlink_metadata(self.legacy_store().join("objects")).is_ok_and(|m| m.is_dir())
    }

    pub fn installer(&self) -> PathBuf {
        self.root.join("installer")
    }

    pub fn humidor(&self) -> PathBuf {
        self.root.join("humidor")
    }

    /// `toolchains/<tool>/<version>`.
    pub fn toolchain(&self, tool: &str, v: &Version) -> PathBuf {
        self.toolchains().join(tool).join(v.to_string())
    }

    pub fn state_path(&self) -> PathBuf {
        self.installer().join("state.json")
    }

    pub fn config_path(&self) -> PathBuf {
        self.installer().join("config")
    }

    pub fn shim_index_path(&self) -> PathBuf {
        self.installer().join("shim-index")
    }

    /// The lock file. On Unix `installer/lock`. On Windows `<home>/installer.lock`, outside
    /// `installer/`: a directory cannot be renamed there while a file inside it is open, and
    /// `uninstall --all` renames `installer/` aside as its commit point with the lock held.
    pub fn lock_path(&self) -> PathBuf {
        if cfg!(windows) {
            self.root.join("installer.lock")
        } else {
            self.installer().join("lock")
        }
    }

    /// The file the holder's pid is readable from. On Unix it is the lock file itself. On
    /// Windows a byte range locked with `LockFileEx` cannot be read through another handle, so
    /// the pid is kept in `installer/lock.pid`, written and closed (no open handle stays in
    /// `installer/`).
    pub fn lock_pid_path(&self) -> PathBuf {
        if cfg!(windows) {
            self.installer().join("lock.pid")
        } else {
            self.lock_path()
        }
    }

    pub fn txn_root(&self) -> PathBuf {
        self.installer().join("txn")
    }

    /// The content-addressed store of this home.
    pub fn object_store(&self) -> Store<'_> {
        Store::new(self)
    }

    /// Create the directories this build owns, if missing: `installer/` and
    /// `humidor/store/` at 0700 on Unix, `bin/`, `toolchains/` and `humidor/` at 0755. Existing
    /// directories are never re-moded.
    pub fn ensure_layout(&self) -> Result<(), HomeError> {
        let made_root = fs::symlink_metadata(&self.root).is_err();
        self.mkdir_mode(&self.root, 0o700)?;
        if made_root {
            // 0700 on Windows: under the user's profile the inherited access list already keeps
            // others out; a home elsewhere gets a list of the current user alone
            platform::restrict_to_owner(&self.root).map_err(|source| HomeError::Io {
                path: self.root.clone(),
                source,
            })?;
        }
        // the home and installer/ are the user's alone: a looser one is tightened
        for p in [self.root.clone(), self.installer()] {
            if platform::mode_of(&p).is_some_and(|m| m & 0o077 != 0) {
                if self.root == p && !p.is_dir() {
                    continue;
                }
                let _ = platform::set_mode(&p, 0o700);
            }
        }
        for (p, mode) in [
            (self.bin(), 0o755),
            (self.toolchains(), 0o755),
            (self.humidor(), 0o755),
            (self.store(), 0o700),
            (self.store().join("objects"), 0o700),
            (self.store().join("tmp"), 0o700),
            (self.installer(), 0o700),
            (self.txn_root(), 0o700),
            (self.installer().join("cache"), 0o700),
            (self.installer().join("cache").join("manifests"), 0o700),
            (self.installer().join("cache").join("downloads"), 0o700),
        ] {
            self.mkdir_mode(&p, mode)?;
        }
        self.write_owner_marker();
        Ok(())
    }

    /// A warning for a home path long enough to run into the Windows path limit (over about 100
    /// characters), `None` elsewhere and for a short path. The caller says it once, up front.
    pub fn long_path_warning(&self) -> Option<String> {
        if cfg!(windows) {
            platform::long_home_message(&self.root)
        } else {
            None
        }
    }

    /// `installer/.owner`: written where the layout is made, so a later `uninstall --all` can tell
    /// a home this installer made from a directory that merely has an `installer/` in it.
    fn write_owner_marker(&self) {
        let p = self.owner_marker();
        if fs::symlink_metadata(&p).is_err() {
            let _ = fs::write(&p, OWNER_TEXT);
            let _ = platform::set_mode(&p, 0o600);
        }
    }

    pub fn owner_marker(&self) -> PathBuf {
        self.installer().join(OWNER_FILE)
    }

    /// The home's own directories that others can read or write (`doctor` reports them; the next
    /// command that lays out the home tightens them).
    pub fn loose_dirs(&self) -> Vec<(PathBuf, u32)> {
        [self.root.clone(), self.installer()]
            .into_iter()
            .filter_map(|p| {
                let m = platform::mode_of(&p)?;
                (p.is_dir() && m & 0o077 != 0).then_some((p, m & 0o777))
            })
            .collect()
    }

    fn mkdir_mode(&self, p: &Path, mode: u32) -> Result<(), HomeError> {
        let io_err = |source: io::Error| HomeError::Io {
            path: p.to_path_buf(),
            source,
        };
        match fs::symlink_metadata(p) {
            Ok(md) if platform::is_link_md(&md, p) && !(p == self.root && p.is_dir()) => {
                return Err(HomeError::Unsafe {
                    path: p.to_path_buf(),
                    why: "a symbolic link where the installer keeps a directory".into(),
                })
            }
            Ok(_) if p == self.root && p.is_dir() => return Ok(()),
            Ok(md) if md.is_dir() => return Ok(()),
            Ok(_) => {
                return Err(HomeError::Unsafe {
                    path: p.to_path_buf(),
                    why: "not a directory".into(),
                })
            }
            Err(_) => {}
        }
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).map_err(io_err)?;
        }
        match fs::create_dir(p) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Ok(()),
            Err(e) => return Err(io_err(e)),
        }
        platform::set_mode(p, mode).map_err(io_err)
    }

    /// Section 4.1: every directory the installer writes into must be owned
    /// by the user (root excepted) and not group- or world-writable.
    /// `E1835` (via [`HomeError::Unsafe`]) otherwise. Missing directories are
    /// skipped: `ensure_layout` creates them safely.
    pub fn check_permissions(&self) -> Result<(), HomeError> {
        let mut dirs = vec![
            self.root.clone(),
            self.bin(),
            self.toolchains(),
            self.store(),
            self.store().join("objects"),
            self.store().join("tmp"),
            self.installer(),
            self.txn_root(),
            self.installer().join("cache"),
            self.installer().join("cache").join("manifests"),
            self.installer().join("cache").join("downloads"),
            self.humidor(),
        ];
        if let Ok(rd) = fs::read_dir(self.toolchains()) {
            for e in rd.flatten() {
                if e.file_name().to_string_lossy().starts_with('.') {
                    continue;
                }
                dirs.push(e.path());
                // the version directories and their subdirectories too
                let mut stack = vec![e.path()];
                while let Some(d) = stack.pop() {
                    let Ok(inner) = fs::read_dir(&d) else {
                        continue;
                    };
                    for i in inner.flatten() {
                        if i.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                            dirs.push(i.path());
                            stack.push(i.path());
                        }
                    }
                }
            }
        }
        for p in dirs {
            // the root itself may be a link the user made on purpose (a moved disk);
            // everything the installer keeps inside it may not be
            if fs::symlink_metadata(&p).is_err() {
                continue;
            }
            let target = if p == self.root {
                p.canonicalize().unwrap_or_else(|_| p.clone())
            } else {
                p.clone()
            };
            let checked = platform::dir_ok(&target);
            checked.map_err(|why| HomeError::Unsafe {
                path: p.clone(),
                why,
            })?;
        }
        Ok(())
    }
}

fn default_user_home(root: &Path) -> PathBuf {
    root.parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| root.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(map: HashMap<&'static str, &'static str>) -> impl Fn(&str) -> Option<OsString> {
        move |k| map.get(k).map(OsString::from)
    }

    #[test]
    fn discovery_order() {
        let h = Home::discover_with(&env(HashMap::from([
            ("CIGSCRIPT_HOME", "/x/cs"),
            ("HOME", "/h"),
        ])))
        .unwrap();
        assert_eq!(h.root(), PathBuf::from("/x/cs"));
        assert_eq!(h.user_home(), PathBuf::from("/h"));
        let h = Home::discover_with(&env(HashMap::from([("HOME", "/h")]))).unwrap();
        assert_eq!(h.root(), PathBuf::from("/h/.cigscript"));
        let h = Home::discover_with(&env(HashMap::from([("USERPROFILE", "C:/u")]))).unwrap();
        assert_eq!(h.root(), PathBuf::from("C:/u/.cigscript"));
        let h = Home::discover_with(&env(HashMap::from([("HOME", ""), ("USERPROFILE", "/u")])))
            .unwrap();
        assert_eq!(h.root(), PathBuf::from("/u/.cigscript"));
        assert!(Home::discover_with(&env(HashMap::new())).is_err());
    }

    #[test]
    fn paths() {
        let h = Home::at(PathBuf::from("/h/.cigscript"));
        let v = Version::parse("1.20.0").unwrap();
        assert_eq!(
            h.toolchain("cig", &v),
            PathBuf::from("/h/.cigscript/toolchains/cig/1.20.0")
        );
        assert_eq!(h.user_home(), PathBuf::from("/h"));
        assert_eq!(
            h.shim_index_path(),
            PathBuf::from("/h/.cigscript/installer/shim-index")
        );
    }

    #[test]
    fn layout_and_permissions() {
        let t = tempfile::tempdir().unwrap();
        let h = Home::at(t.path().join(".cigscript"));
        h.ensure_layout().unwrap();
        h.ensure_layout().unwrap();
        assert!(h.store().join("objects").is_dir());
        assert!(h.txn_root().is_dir());
        h.check_permissions().unwrap();
        if let Some(m) = platform::mode_of(&h.installer()) {
            assert_eq!(m, 0o700);
        }
        if platform::mode_of(&h.bin()).is_some() {
            platform::set_mode(&h.bin(), 0o777).unwrap();
            let e = h.check_permissions().unwrap_err();
            assert_eq!(e.code(), "E1835");
            platform::set_mode(&h.bin(), 0o755).unwrap();
            h.check_permissions().unwrap();
        }
    }
}
