// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! The user's `PATH` in `HKCU\Environment`, the `WM_SETTINGCHANGE` broadcast, and the Documents folder.
//!
//! The value is read raw: it is never expanded, its type is kept (usually `REG_EXPAND_SZ`), and
//! text that is not valid UTF-16 is refused rather than rewritten. Nothing here shells out to
//! `setx`, which truncates at 1024 characters and turns a `REG_EXPAND_SZ` into a `REG_SZ`.

use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use std::ptr;

use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_SUCCESS, S_OK};
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_EXPAND_SZ, REG_SZ,
};
use windows_sys::Win32::UI::Shell::{FOLDERID_Documents, SHGetKnownFolderPath};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
};

/// How long the `WM_SETTINGCHANGE` broadcast waits for each top-level window, in milliseconds.
pub const BROADCAST_TIMEOUT_MS: u32 = 5000;

/// The registry type of the user's `Path`. Any other type is refused, not converted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathKind {
    /// `REG_SZ`
    Sz,
    /// `REG_EXPAND_SZ`: holds `%VAR%` references, and is what Windows itself writes.
    ExpandSz,
}

impl PathKind {
    /// The registry's name for the type.
    pub fn name(self) -> &'static str {
        match self {
            PathKind::Sz => "REG_SZ",
            PathKind::ExpandSz => "REG_EXPAND_SZ",
        }
    }
}

/// The user's `Path` exactly as stored: type and unexpanded text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserPath {
    pub kind: PathKind,
    pub text: String,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn win_err(code: u32) -> io::Error {
    io::Error::from_raw_os_error(code as i32)
}

/// An open `HKCU\Environment`, closed on drop.
struct EnvKey(HKEY);

impl EnvKey {
    fn open(write: bool) -> io::Result<EnvKey> {
        let sub = wide("Environment");
        let access = if write {
            KEY_QUERY_VALUE | KEY_SET_VALUE
        } else {
            KEY_QUERY_VALUE
        };
        let mut key: HKEY = ptr::null_mut();
        // SAFETY: `sub` is a NUL-terminated UTF-16 string that outlives the call, `key` is a
        // valid out-pointer, and HKEY_CURRENT_USER is a predefined key handle.
        let rc = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, sub.as_ptr(), 0, access, &mut key) };
        if rc != ERROR_SUCCESS {
            return Err(win_err(rc));
        }
        Ok(EnvKey(key))
    }
}

impl Drop for EnvKey {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful RegOpenKeyExW and is closed only here.
        unsafe {
            RegCloseKey(self.0);
        }
    }
}

/// The user's `Path` from `HKCU\Environment`, raw. `None` when the value does not exist.
pub fn read_user_path() -> io::Result<Option<UserPath>> {
    let key = EnvKey::open(false)?;
    let name = wide("Path");
    let mut cap: u32 = 4096;
    loop {
        // UTF-16 units, so the buffer is aligned for them; the call counts bytes
        let mut buf = vec![0u16; (cap as usize).div_ceil(2)];
        let mut len = (buf.len() * 2) as u32;
        let mut ty: u32 = 0;
        // SAFETY: `name` is NUL-terminated; `buf` is `len` writable bytes (as the call is told); `ty`
        // and `len` are valid out-pointers; the key is open for the whole call.
        let rc = unsafe {
            RegQueryValueExW(
                key.0,
                name.as_ptr(),
                ptr::null(),
                &mut ty,
                buf.as_mut_ptr().cast::<u8>(),
                &mut len,
            )
        };
        if rc == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        if rc == ERROR_MORE_DATA {
            // the value grew between the size ask and the read: try again with room
            cap = len.max(cap.saturating_mul(2));
            continue;
        }
        if rc != ERROR_SUCCESS {
            return Err(win_err(rc));
        }
        let kind = match ty {
            REG_SZ => PathKind::Sz,
            REG_EXPAND_SZ => PathKind::ExpandSz,
            other => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                    "HKCU\\Environment\\Path has registry type {other}, not a string; left alone"
                ),
                ))
            }
        };
        buf.truncate(len as usize / 2);
        let mut units = buf;
        while units.last() == Some(&0) {
            units.pop();
        }
        let text = String::from_utf16(&units).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "HKCU\\Environment\\Path is not valid Unicode text; left alone",
            )
        })?;
        return Ok(Some(UserPath { kind, text }));
    }
}

/// Write the user's `Path` with the given type. The text is stored as given, never expanded.
pub fn write_user_path(kind: PathKind, text: &str) -> io::Result<()> {
    let key = EnvKey::open(true)?;
    let name = wide("Path");
    let data = wide(text);
    let bytes = data
        .len()
        .checked_mul(2)
        .and_then(|n| u32::try_from(n).ok());
    let Some(bytes) = bytes else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the PATH value is too long for the registry",
        ));
    };
    let ty = match kind {
        PathKind::Sz => REG_SZ,
        PathKind::ExpandSz => REG_EXPAND_SZ,
    };
    // SAFETY: `name` is NUL-terminated; `data` is `bytes` readable bytes (its NUL included, as
    // REG_SZ and REG_EXPAND_SZ want); the key is open with KEY_SET_VALUE.
    let rc = unsafe {
        RegSetValueExW(
            key.0,
            name.as_ptr(),
            0,
            ty,
            data.as_ptr().cast::<u8>(),
            bytes,
        )
    };
    if rc != ERROR_SUCCESS {
        return Err(win_err(rc));
    }
    Ok(())
}

/// Delete the user's `Path` value. Succeeds when it is already gone.
pub fn delete_user_path() -> io::Result<()> {
    let key = EnvKey::open(true)?;
    let name = wide("Path");
    // SAFETY: `name` is NUL-terminated and the key is open with KEY_SET_VALUE.
    let rc = unsafe { RegDeleteValueW(key.0, name.as_ptr()) };
    if rc != ERROR_SUCCESS && rc != ERROR_FILE_NOT_FOUND {
        return Err(win_err(rc));
    }
    Ok(())
}

/// Tell running programs (Explorer, so every terminal opened after) that the environment changed:
/// `WM_SETTINGCHANGE` with "Environment", `SMTO_ABORTIFHUNG`, 5000 ms per window. `true` when the
/// broadcast went through; a window that did not answer in time is not an error, and the registry
/// is already right either way, so `false` only means "a new terminal may need a sign-out".
pub fn broadcast_environment_change() -> bool {
    let param = wide("Environment");
    let mut result: usize = 0;
    // SAFETY: `param` is a NUL-terminated UTF-16 string that outlives the call (receivers copy it
    // before the call returns); HWND_BROADCAST is the documented broadcast target; `result` is a
    // valid out-pointer.
    let rc = unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            0,
            param.as_ptr() as isize,
            SMTO_ABORTIFHUNG,
            BROADCAST_TIMEOUT_MS,
            &mut result,
        )
    };
    rc != 0
}

/// The user's Documents folder, where OneDrive redirection and a moved folder are honored.
pub fn documents_dir() -> io::Result<PathBuf> {
    let mut out: *mut u16 = ptr::null_mut();
    // SAFETY: FOLDERID_Documents is a valid GUID constant, flags 0 and a null token mean "the
    // current user, no creation", and `out` is a valid out-pointer.
    let hr = unsafe { SHGetKnownFolderPath(&FOLDERID_Documents, 0, ptr::null_mut(), &mut out) };
    let result = if hr == S_OK && !out.is_null() {
        // SAFETY: on S_OK `out` points at a NUL-terminated UTF-16 string, so scanning to the NUL
        // stays inside the allocation, and the slice is read before the string is freed below.
        let path = unsafe {
            let mut n = 0usize;
            while *out.add(n) != 0 {
                n += 1;
            }
            PathBuf::from(OsString::from_wide(std::slice::from_raw_parts(out, n)))
        };
        Ok(path)
    } else {
        Err(io::Error::from_raw_os_error(hr))
    };
    // SAFETY: the caller owns the string SHGetKnownFolderPath returned, even on failure, and it
    // is freed exactly once, with CoTaskMemFree (a null pointer is allowed).
    unsafe { CoTaskMemFree(out.cast()) };
    result
}
