// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! The values of one key under `HKEY_LOCAL_MACHINE`, read only: the organization's settings in
//! `HKLM\SOFTWARE\Policies\otmof-ops\CigScript`. The key is opened for querying in the 64-bit
//! view; nothing here creates, writes or deletes anything.
//!
//! Every value is returned with its type kept (`REG_SZ` and `REG_EXPAND_SZ` as text, never
//! expanded; `REG_DWORD`; `REG_MULTI_SZ` as a list), so the caller can tell a malformed value
//! from a missing one. Text that is not valid UTF-16 is an error, not a lossy guess.

use std::io;
use std::ptr;

use windows_sys::Win32::Foundation::{
    ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS,
};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegEnumValueW, RegOpenKeyExW, HKEY, HKEY_LOCAL_MACHINE, KEY_QUERY_VALUE,
    KEY_WOW64_64KEY, REG_DWORD, REG_EXPAND_SZ, REG_MULTI_SZ, REG_SZ,
};

/// One registry value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyValue {
    /// `REG_SZ` or `REG_EXPAND_SZ`.
    Text(String),
    /// `REG_DWORD`.
    Number(u32),
    /// `REG_MULTI_SZ`, without the empty strings that end it.
    List(Vec<String>),
    /// Any other type, by its number.
    Other(u32),
}

/// The longest value name the registry allows, in UTF-16 units, plus the NUL.
const MAX_NAME: usize = 16_384;
/// The largest value read; a policy value is a short word or a short list.
const MAX_DATA: usize = 64 * 1024;
/// The most values read from one key.
const MAX_VALUES: u32 = 512;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn win_err(code: u32) -> io::Error {
    io::Error::from_raw_os_error(code as i32)
}

struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful RegOpenKeyExW and is closed only here.
        unsafe {
            RegCloseKey(self.0);
        }
    }
}

fn utf16(data: &[u8]) -> io::Result<Vec<u16>> {
    let (pairs, rest) = data.as_chunks::<2>();
    if !rest.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "a text value has an odd number of bytes",
        ));
    }
    Ok(pairs.iter().map(|b| u16::from_le_bytes(*b)).collect())
}

fn text(data: &[u8]) -> io::Result<String> {
    let mut u = utf16(data)?;
    while u.last() == Some(&0) {
        u.pop();
    }
    String::from_utf16(&u).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "a text value is not valid UTF-16",
        )
    })
}

fn list(data: &[u8]) -> io::Result<Vec<String>> {
    let u = utf16(data)?;
    let mut out = Vec::new();
    for part in u.split(|c| *c == 0) {
        if part.is_empty() {
            continue;
        }
        out.push(String::from_utf16(part).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "a list value is not valid UTF-16",
            )
        })?);
    }
    Ok(out)
}

fn value_of(ty: u32, data: &[u8]) -> io::Result<PolicyValue> {
    Ok(match ty {
        REG_SZ | REG_EXPAND_SZ => PolicyValue::Text(text(data)?),
        REG_DWORD if data.len() == 4 => {
            PolicyValue::Number(u32::from_le_bytes([data[0], data[1], data[2], data[3]]))
        }
        REG_DWORD => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "a number value is not four bytes",
            ))
        }
        REG_MULTI_SZ => PolicyValue::List(list(data)?),
        other => PolicyValue::Other(other),
    })
}

/// Every value of `HKLM\<subkey>`, in the registry's order. `Ok(None)` when the key does not
/// exist. An error names the failing call's code or the value that could not be read.
pub fn read_values(subkey: &str) -> io::Result<Option<Vec<(String, PolicyValue)>>> {
    let sub = wide(subkey);
    let mut handle: HKEY = ptr::null_mut();
    // SAFETY: `sub` is a NUL-terminated UTF-16 string that outlives the call, `handle` is a valid
    // out-pointer, and HKEY_LOCAL_MACHINE is a predefined key handle.
    let rc = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            sub.as_ptr(),
            0,
            KEY_QUERY_VALUE | KEY_WOW64_64KEY,
            &mut handle,
        )
    };
    if rc == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if rc != ERROR_SUCCESS {
        return Err(win_err(rc));
    }
    let key = Key(handle);
    let mut out = Vec::new();
    let mut name = vec![0u16; MAX_NAME];
    let mut data = vec![0u8; 4096];
    for index in 0..MAX_VALUES {
        loop {
            let mut name_len = name.len() as u32;
            let mut data_len = data.len() as u32;
            let mut ty: u32 = 0;
            // SAFETY: the key is open for the whole call; `name` and `data` are writable for the
            // lengths passed in `name_len` (in UTF-16 units) and `data_len` (in bytes); `ty` is a
            // valid out-pointer; the reserved argument is null as documented.
            let rc = unsafe {
                RegEnumValueW(
                    key.0,
                    index,
                    name.as_mut_ptr(),
                    &mut name_len,
                    ptr::null(),
                    &mut ty,
                    data.as_mut_ptr(),
                    &mut data_len,
                )
            };
            if rc == ERROR_NO_MORE_ITEMS {
                return Ok(Some(out));
            }
            if rc == ERROR_MORE_DATA {
                let want = data_len as usize;
                if want > MAX_DATA {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "a value is larger than a setting can be",
                    ));
                }
                data.resize(want.max(data.len() * 2).min(MAX_DATA), 0);
                continue;
            }
            if rc != ERROR_SUCCESS {
                return Err(win_err(rc));
            }
            let n = String::from_utf16(&name[..name_len as usize]).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "a value name is not valid UTF-16",
                )
            })?;
            let v = value_of(ty, &data[..data_len as usize])
                .map_err(|e| io::Error::new(e.kind(), format!("`{n}`: {e}")))?;
            out.push((n, v));
            break;
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "the key holds more values than a setting list can",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    #[test]
    fn text_drops_the_terminator_and_keeps_the_rest() {
        let mut d = bytes("1.1");
        d.extend([0, 0]);
        assert_eq!(
            value_of(REG_SZ, &d).unwrap(),
            PolicyValue::Text("1.1".into())
        );
    }

    #[test]
    fn a_multi_string_is_a_list() {
        let mut d = bytes("carton");
        d.extend([0, 0]);
        d.extend(bytes("mirror"));
        d.extend([0, 0, 0, 0]);
        assert_eq!(
            value_of(REG_MULTI_SZ, &d).unwrap(),
            PolicyValue::List(vec!["carton".into(), "mirror".into()])
        );
    }

    #[test]
    fn a_dword_must_be_four_bytes_and_odd_text_is_refused() {
        assert_eq!(
            value_of(REG_DWORD, &[7, 0, 0, 0]).unwrap(),
            PolicyValue::Number(7)
        );
        assert!(value_of(REG_DWORD, &[7, 0]).is_err());
        assert!(value_of(REG_SZ, &[1, 2, 3]).is_err());
    }
}
