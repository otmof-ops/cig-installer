// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! #5, PATH: the profile blocks for both PowerShells (every platform) and, on Windows, the user
//! PATH in `HKCU\Environment` (the `win_*` tests run only on the Windows runner).

mod common;

use cigi_testkit::Fixture;
#[cfg(not(windows))]
use common::cigi;
use common::{fixture, ok};

fn one_carton(fx: &mut Fixture) {
    fx.carton("0.1.0", &[("cig", "1.20.0"), ("cigo", "1.16.0")]);
    fx.publish("stable");
}

const BEGIN: &str = "# >>> cigscript >>>";

fn state(fx: &Fixture) -> serde_json::Value {
    let t = std::fs::read_to_string(fx.cigscript().join("installer/state.json")).unwrap();
    serde_json::from_str(&t).unwrap()
}

/// Windows PowerShell 5.1 has a profile of its own, beside pwsh 7's: `--shell powershell` writes it
/// (on Windows, both PowerShells get a block whatever `--shell` says; here the one asked for).
#[cfg(not(windows))]
#[test]
fn a2_path_block_windows_powershell_profile() {
    let mut fx = fixture();
    one_carton(&mut fx);
    let ps5 = fx
        .user_home
        .join("Documents/WindowsPowerShell/Microsoft.PowerShell_profile.ps1");
    ok(&cigi(
        &fx,
        &[
            "install",
            "carton",
            "--shell",
            "powershell",
            "--carton",
            "0.1.0",
        ],
    ));
    let t = std::fs::read_to_string(&ps5).unwrap();
    assert_eq!(t.matches(BEGIN).count(), 1, "{t}");
    assert!(t.contains("$env:PATH"), "{t}");
    // a second install changes nothing
    ok(&cigi(
        &fx,
        &[
            "install",
            "carton",
            "--shell",
            "powershell",
            "--carton",
            "0.1.0",
        ],
    ));
    assert_eq!(std::fs::read_to_string(&ps5).unwrap(), t);
    // the record names the shell, and the registry is not a thing here
    let s = state(&fx);
    assert_eq!(s["path_block"]["shells"][0], "windowspowershell", "{s}");
    assert!(s.get("path_registry").is_none(), "{s}");
    // uninstall --all takes the block, and the file the install made
    ok(&cigi(&fx, &["uninstall", "--all", "--yes-irreversible"]));
    assert!(!ps5.exists());
}

/// pwsh 7 and Windows PowerShell 5.1 are different profiles: a block for one does not touch the
/// other, and uninstall finds each.
#[cfg(not(windows))]
#[test]
fn a2_path_block_both_powershells_are_separate_files() {
    let mut fx = fixture();
    one_carton(&mut fx);
    let ps7 = fx
        .user_home
        .join(".config/powershell/Microsoft.PowerShell_profile.ps1");
    let ps5 = fx
        .user_home
        .join("Documents/WindowsPowerShell/Microsoft.PowerShell_profile.ps1");
    ok(&cigi(
        &fx,
        &["install", "carton", "--shell", "pwsh", "--carton", "0.1.0"],
    ));
    assert!(ps7.exists() && !ps5.exists());
    ok(&cigi(&fx, &["uninstall", "--all", "--yes-irreversible"]));
    assert!(!ps7.exists());
}

/// A hand-edited block in the 5.1 profile is left alone and reported (E1862), as for any shell.
#[cfg(not(windows))]
#[test]
fn a2_path_block_edited_windows_powershell_block_is_left_alone() {
    let mut fx = fixture();
    one_carton(&mut fx);
    let ps5 = fx
        .user_home
        .join("Documents/WindowsPowerShell/Microsoft.PowerShell_profile.ps1");
    ok(&cigi(
        &fx,
        &[
            "install",
            "carton",
            "--shell",
            "powershell",
            "--carton",
            "0.1.0",
        ],
    ));
    let edited = std::fs::read_to_string(&ps5)
        .unwrap()
        .replace("$env:PATH =", "$env:PATH  =");
    std::fs::write(&ps5, &edited).unwrap();
    ok(&cigi(
        &fx,
        &[
            "install",
            "carton",
            "--shell",
            "powershell",
            "--carton",
            "0.1.0",
        ],
    ));
    assert_eq!(std::fs::read_to_string(&ps5).unwrap(), edited);
}

// ---------------------------------------------------------------------------------------- Windows

#[cfg(windows)]
mod windows {
    use super::*;
    use cigi_home::{
        path::{documents_dir, same_entry},
        registry_path_add, registry_path_remove, RegistryChange, Shell,
    };
    use cigi_sys::env::{delete_user_path, read_user_path, write_user_path, PathKind, UserPath};
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::Mutex;

    /// The registry is one per user, so the tests that write it take turns.
    static REGISTRY: Mutex<()> = Mutex::new(());

    /// Puts the user's `Path` back as it was found, even when a test fails.
    struct Restore(Option<UserPath>);

    impl Restore {
        fn take() -> Restore {
            Restore(read_user_path().expect("read HKCU\\Environment\\Path"))
        }
    }

    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = match &self.0 {
                Some(p) => write_user_path(p.kind, &p.text),
                None => delete_user_path(),
            };
            cigi_sys::env::broadcast_environment_change();
        }
    }

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        REGISTRY.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A scratch directory name no real PATH holds.
    fn entry(tag: &str) -> PathBuf {
        PathBuf::from(format!(
            r"C:\cigi-test-{tag}-{}\.cigscript\bin",
            std::process::id()
        ))
    }

    #[test]
    fn win_registry_add_keeps_type_and_text_and_remove_restores_them() {
        let _g = lock();
        let _restore = Restore::take();
        let base = r"%USERPROFILE%\bin;C:\Tools\;;D:\x";
        write_user_path(PathKind::ExpandSz, base).unwrap();
        let e = entry("a");
        assert_eq!(registry_path_add(&e).unwrap(), RegistryChange::Added);
        let now = read_user_path().unwrap().unwrap();
        assert_eq!(now.kind, PathKind::ExpandSz, "the type is kept");
        assert_eq!(
            now.text,
            format!("{};{base}", e.display()),
            "prepended, unexpanded"
        );
        // idempotent, and case and a trailing backslash do not matter
        assert_eq!(
            registry_path_add(&e).unwrap(),
            RegistryChange::AlreadyPresent
        );
        let shouty = PathBuf::from(format!(r"{}\", e.display().to_string().to_uppercase()));
        assert_eq!(
            registry_path_add(&shouty).unwrap(),
            RegistryChange::AlreadyPresent
        );
        assert_eq!(
            read_user_path()
                .unwrap()
                .unwrap()
                .text
                .matches("cigi-test-a")
                .count(),
            1
        );
        // removal gives back the value byte for byte
        assert_eq!(registry_path_remove(&e).unwrap(), RegistryChange::Removed);
        let back = read_user_path().unwrap().unwrap();
        assert_eq!((back.kind, back.text.as_str()), (PathKind::ExpandSz, base));
        assert_eq!(
            registry_path_remove(&e).unwrap(),
            RegistryChange::NotPresent
        );
    }

    #[test]
    fn win_registry_reg_sz_stays_reg_sz() {
        let _g = lock();
        let _restore = Restore::take();
        write_user_path(PathKind::Sz, r"C:\Tools").unwrap();
        let e = entry("b");
        registry_path_add(&e).unwrap();
        assert_eq!(read_user_path().unwrap().unwrap().kind, PathKind::Sz);
        registry_path_remove(&e).unwrap();
        let back = read_user_path().unwrap().unwrap();
        assert_eq!((back.kind, back.text.as_str()), (PathKind::Sz, r"C:\Tools"));
    }

    #[test]
    fn win_registry_a_missing_path_is_made_and_unmade() {
        let _g = lock();
        let _restore = Restore::take();
        delete_user_path().unwrap();
        let e = entry("c");
        assert_eq!(registry_path_add(&e).unwrap(), RegistryChange::Added);
        let now = read_user_path().unwrap().unwrap();
        assert_eq!(now.kind, PathKind::ExpandSz);
        assert_eq!(now.text, e.display().to_string());
        registry_path_remove(&e).unwrap();
        assert_eq!(
            read_user_path().unwrap(),
            None,
            "the value we made is gone again"
        );
    }

    #[test]
    fn win_documents_dir_is_the_known_folder() {
        let d = cigi_sys::env::documents_dir().unwrap();
        assert!(d.is_dir(), "{}", d.display());
        // the real user's home uses it, so a OneDrive-redirected Documents is honored
        let home = cigi_home::Home::at(PathBuf::from(r"C:\x\.cigscript"))
            .with_user_home(PathBuf::from(std::env::var_os("USERPROFILE").unwrap()));
        assert_eq!(documents_dir(&home), d);
        // any other home (a fixture's) gets <home>\Documents
        let other = cigi_home::Home::at(PathBuf::from(r"C:\x\.cigscript"))
            .with_user_home(PathBuf::from(r"C:\somebody-else"));
        assert_eq!(
            documents_dir(&other),
            PathBuf::from(r"C:\somebody-else\Documents")
        );
    }

    fn installer(fx: &Fixture, args: &[&str]) -> std::process::Output {
        // the one place the registry is meant to be written
        fx.cigi()
            .env("CIGI_ALLOW_SINGLE_SOURCE", "true")
            .env_remove("CIGI_NO_REGISTRY_PATH")
            .arg("--yes")
            .args(args)
            .output()
            .unwrap()
    }

    fn profiles(fx: &Fixture) -> [PathBuf; 2] {
        [
            fx.user_home
                .join(r"Documents\PowerShell\Microsoft.PowerShell_profile.ps1"),
            fx.user_home
                .join(r"Documents\WindowsPowerShell\Microsoft.PowerShell_profile.ps1"),
        ]
    }

    /// What a child PowerShell printed. The child inherits THIS process's environment, so it
    /// never sees a registry change by itself: every claim below is made by reading the registry
    /// or the profile in the child, not by hoping the environment was refreshed.
    struct Ran {
        out: String,
        err: String,
    }

    /// Run `script` in a PowerShell. `None` only when the program is not installed.
    fn ps(exe: &str, script: &str) -> Option<Ran> {
        let o = Command::new(exe)
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                script,
            ])
            .output()
            .ok()?;
        Some(Ran {
            out: String::from_utf8_lossy(&o.stdout).trim().to_string(),
            err: String::from_utf8_lossy(&o.stderr).trim().to_string(),
        })
    }

    /// pwsh 7 may be missing from a machine; Windows PowerShell 5.1 never is.
    fn shells() -> Vec<(&'static str, Shell)> {
        let mut v = vec![("powershell", Shell::WindowsPowerShell)];
        if ps("pwsh", "1").is_some() {
            v.insert(0, ("pwsh", Shell::PowerShell));
        }
        v
    }

    /// A child reads the user PATH from the registry (what a fresh sign-in starts from): the
    /// entry is first, and it is a directory holding `cigi.exe`.
    fn assert_registry_first(exe: &str, fx: &Fixture) {
        let bin = fx.cigscript().join("bin");
        let r = ps(exe, "[Environment]::GetEnvironmentVariable('Path','User')")
            .unwrap_or_else(|| panic!("{exe} did not start"));
        let first = r.out.split(';').next().unwrap_or("").to_string();
        assert!(
            same_entry(&first, &bin.to_string_lossy()),
            "{exe}: the user PATH starts with {first:?}, not {} (stderr {:?}; whole {:?})",
            bin.display(),
            r.err,
            r.out
        );
        let has = ps(
            exe,
            &format!("Test-Path -LiteralPath (Join-Path '{first}' 'cigi.exe')"),
        )
        .unwrap();
        assert_eq!(
            has.out, "True",
            "{exe}: no cigi.exe in {first} ({})",
            has.err
        );
    }

    /// The whole #5 round trip on Windows: install writes the registry entry (first, type kept,
    /// recorded in state.json) and both profiles; a fresh pwsh 7, Windows PowerShell 5.1 and
    /// cmd.exe resolve `cigi` from it; `uninstall --all` gives back the registry value exactly and
    /// removes the profiles.
    #[test]
    fn win_install_puts_cigi_on_the_user_path_and_uninstall_restores_it() {
        let _g = lock();
        let restore = Restore::take();
        let before = restore.0.clone();
        let mut fx = fixture();
        one_carton(&mut fx);
        ok(&installer(&fx, &["install", "carton", "--carton", "0.1.0"]));
        let bin = fx.cigscript().join("bin");

        let now = read_user_path().unwrap().unwrap();
        assert!(
            now.text
                .to_lowercase()
                .starts_with(&bin.display().to_string().to_lowercase()),
            "{}",
            now.text
        );
        if let Some(b) = &before {
            assert_eq!(now.kind, b.kind, "the type is kept");
            assert!(now.text.ends_with(&b.text));
        }
        let s = state(&fx);
        assert_eq!(s["path_registry"]["preexisting"], false, "{s}");
        assert_eq!(
            s["path_registry"]["entry"].as_str().unwrap().to_lowercase(),
            bin.display().to_string().to_lowercase()
        );
        for p in profiles(&fx) {
            let t = std::fs::read_to_string(&p).unwrap_or_default();
            assert_eq!(t.matches(BEGIN).count(), 1, "{}: {t}", p.display());
        }

        // a second install adds nothing and keeps the record
        ok(&installer(&fx, &["install", "carton", "--carton", "0.1.0"]));
        assert_eq!(
            read_user_path()
                .unwrap()
                .unwrap()
                .text
                .to_lowercase()
                .matches(&bin.display().to_string().to_lowercase())
                .count(),
            1
        );
        assert_eq!(state(&fx)["path_registry"]["preexisting"], false);

        // fresh shells start from the registry: read it in each, the entry is first
        for (exe, _) in shells() {
            assert_registry_first(exe, &fx);
        }

        // uninstall: the registry value is what it was, the profiles are gone
        ok(&installer(
            &fx,
            &["uninstall", "--all", "--yes-irreversible"],
        ));
        let after = read_user_path().unwrap();
        assert_eq!(
            after.as_ref().map(|p| (p.kind, p.text.as_str())),
            before.as_ref().map(|p| (p.kind, p.text.as_str()))
        );
        for p in profiles(&fx) {
            assert!(!p.exists(), "{}", p.display());
        }
    }

    /// The profile blocks: each PowerShell loads the profile path the installer writes for the
    /// real user (asked of the shell itself), and running the block we wrote puts the home's `bin`
    /// first on PATH. The fixture's profiles are under its own home, not the runner's real
    /// Documents, so the block is run by dot-sourcing the exact file.
    #[test]
    fn win_profile_blocks_put_cigi_on_the_path_of_a_fresh_powershell() {
        let _g = lock();
        let _restore = Restore::take();
        let mut fx = fixture();
        one_carton(&mut fx);
        ok(&installer(&fx, &["install", "carton", "--carton", "0.1.0"]));
        let bin = fx.cigscript().join("bin");
        let real = cigi_home::Home::at(PathBuf::from(r"C:\x\.cigscript"))
            .with_user_home(PathBuf::from(std::env::var_os("USERPROFILE").unwrap()));
        let [ps7, ps5] = profiles(&fx);
        for (exe, shell) in shells() {
            // 1. the path pwsh really loads is the one the installer would write for this user
            let loaded = ps(exe, "$PROFILE.CurrentUserCurrentHost").unwrap();
            let ours = cigi_home::path_block(shell, &real).0;
            assert!(
                same_entry(&loaded.out, &ours.to_string_lossy()),
                "{exe} loads {:?}, the installer writes {} ({})",
                loaded.out,
                ours.display(),
                loaded.err
            );
            // 2. the block in the profile file we wrote puts bin first, from a bare PATH
            let file = if shell == Shell::PowerShell {
                &ps7
            } else {
                &ps5
            };
            let r = ps(
                exe,
                &format!(
                    "$env:Path = 'C:\\Windows\\System32;C:\\Windows'; . '{}'; ($env:Path -split ';')[0]",
                    file.display()
                ),
            )
            .unwrap();
            assert!(
                same_entry(&r.out, &bin.to_string_lossy()),
                "{exe}: after the profile PATH starts with {:?}, not {} (stderr {:?})",
                r.out,
                bin.display(),
                r.err
            );
        }
    }

    /// A user who already had the directory on PATH keeps it: it is not the installer's to remove.
    #[test]
    fn win_uninstall_leaves_an_entry_that_was_there_before() {
        let _g = lock();
        let _restore = Restore::take();
        let mut fx = fixture();
        one_carton(&mut fx);
        let bin = fx.cigscript().join("bin");
        write_user_path(PathKind::ExpandSz, &format!(r"{};C:\Tools", bin.display())).unwrap();
        ok(&installer(&fx, &["install", "carton", "--carton", "0.1.0"]));
        assert_eq!(state(&fx)["path_registry"]["preexisting"], true);
        ok(&installer(
            &fx,
            &["uninstall", "--all", "--yes-irreversible"],
        ));
        let t = read_user_path().unwrap().unwrap().text;
        assert_eq!(t, format!(r"{};C:\Tools", bin.display()));
    }

    /// An entry that went missing is found by doctor (E1845) and put back by repair.
    #[test]
    fn win_doctor_finds_a_missing_registry_entry_and_repair_adds_it() {
        let _g = lock();
        let _restore = Restore::take();
        let mut fx = fixture();
        one_carton(&mut fx);
        ok(&installer(&fx, &["install", "carton", "--carton", "0.1.0"]));
        let bin = fx.cigscript().join("bin");
        registry_path_remove(&bin).unwrap();
        let d = installer(&fx, &["--plain", "doctor"]);
        assert!(common::text(&d).contains("E1845"), "{}", common::text(&d));
        ok(&installer(&fx, &["repair"]));
        assert!(cigi_home::registry_path_has(&bin).unwrap());
    }

    /// `--no-path` touches neither the registry nor the profiles.
    #[test]
    fn win_no_path_leaves_the_registry_alone() {
        let _g = lock();
        let restore = Restore::take();
        let before = restore.0.clone();
        let mut fx = fixture();
        one_carton(&mut fx);
        ok(&installer(
            &fx,
            &["install", "carton", "--no-path", "--carton", "0.1.0"],
        ));
        assert_eq!(read_user_path().unwrap(), before);
        assert!(state(&fx).get("path_registry").is_none());
    }
}
