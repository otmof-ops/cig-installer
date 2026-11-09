// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0

//! `Home`: where everything lives, and the layout it must have (sections 4, 4.1).

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use cig_compat::Version;

use crate::error::HomeError;
use crate::platform;
use crate::store::Store;

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

    pub fn store(&self) -> PathBuf {
        self.root.join("store")
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

    pub fn lock_path(&self) -> PathBuf {
        self.installer().join("lock")
    }

    pub fn txn_root(&self) -> PathBuf {
        self.installer().join("txn")
    }

    /// The content-addressed store of this home.
    pub fn object_store(&self) -> Store<'_> {
        Store::new(self)
    }

    /// Create the directories this build owns, if missing: `installer/` and
    /// `store/` at 0700 on Unix, `bin/` and `toolchains/` at 0755. Existing
    /// directories are never re-moded.
    pub fn ensure_layout(&self) -> Result<(), HomeError> {
        self.mkdir_mode(&self.root, 0o700)?;
        for (p, mode) in [
            (self.bin(), 0o755),
            (self.toolchains(), 0o755),
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
        Ok(())
    }

    fn mkdir_mode(&self, p: &Path, mode: u32) -> Result<(), HomeError> {
        let io_err = |source: io::Error| HomeError::Io {
            path: p.to_path_buf(),
            source,
        };
        match fs::symlink_metadata(p) {
            Ok(md) if md.file_type().is_symlink() && !(p == self.root && p.is_dir()) => {
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
