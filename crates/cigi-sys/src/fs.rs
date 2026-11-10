// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! File identity, attributes, reparse points, ownership and access control, and the write-through rename.
//!
//! Paths come in as `&Path` and go to the system as wide strings with a terminating NUL. The
//! caller decides about `\\?\` long paths (`cigi-home` normalizes first); nothing here rewrites a
//! path. Every function reports a failure as the `io::Error` of the system's last error, so the
//! caller can look at `raw_os_error` (5, 32 and 33 are the ones worth a retry).

use std::ffi::c_void;
use std::fs::OpenOptions;
use std::io;
use std::mem::zeroed;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{
    CloseHandle, LocalFree, ERROR_SUCCESS, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Security::Authorization::{
    GetNamedSecurityInfoW, SetEntriesInAclW, SetNamedSecurityInfoW, EXPLICIT_ACCESS_W,
    NO_MULTIPLE_TRUSTEE, SET_ACCESS, SE_FILE_OBJECT, TRUSTEE_IS_SID, TRUSTEE_IS_USER, TRUSTEE_W,
};
use windows_sys::Win32::Security::{
    EqualSid, GetTokenInformation, IsWellKnownSid, TokenOwner, TokenUser,
    WinBuiltinAdministratorsSid, WinLocalSystemSid, DACL_SECURITY_INFORMATION,
    OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
    SUB_CONTAINERS_AND_OBJECTS_INHERIT, TOKEN_OWNER, TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::Storage::FileSystem::{
    FindClose, FindFirstFileW, GetFileInformationByHandle, MoveFileExW, BY_HANDLE_FILE_INFORMATION,
    FILE_ALL_ACCESS, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, WIN32_FIND_DATAW,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// A NUL-terminated wide copy of a path.
fn wide(path: &Path) -> io::Result<Vec<u16>> {
    let mut w: Vec<u16> = path.as_os_str().encode_wide().collect();
    if w.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a path with a NUL in it",
        ));
    }
    w.push(0);
    Ok(w)
}

// ------------------------------------------------------------------ identity

/// What `GetFileInformationByHandle` says about one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileInfo {
    /// The volume serial number. With `index` it names one file on the machine.
    pub volume: u32,
    /// The 64-bit file index (NTFS: record number and sequence).
    pub index: u64,
    /// Hard links to the file.
    pub links: u32,
    /// `dwFileAttributes`.
    pub attributes: u32,
}

/// Identity, link count and attributes of the file or directory at `path`, links followed. The
/// file is opened for attribute access only, with every share mode, so a program that holds the
/// file open does not get in the way.
pub fn file_info(path: &Path) -> io::Result<FileInfo> {
    let f = OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        // a directory cannot be opened without this
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?;
    // SAFETY: an all-zero BY_HANDLE_FILE_INFORMATION is a valid value (plain integers and
    // FILETIME pairs), and it is only read after the call has filled it in.
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { zeroed() };
    // SAFETY: `f` owns an open file handle for the whole call, and `info` is a live, writable
    // BY_HANDLE_FILE_INFORMATION that the call fills in.
    let ok = unsafe { GetFileInformationByHandle(f.as_raw_handle() as HANDLE, &mut info) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(FileInfo {
        volume: info.dwVolumeSerialNumber,
        index: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
        links: info.nNumberOfLinks,
        attributes: info.dwFileAttributes,
    })
}

// -------------------------------------------------------------- reparse points

/// The reparse tag of the entry at `path` itself (a link is not followed), `None` when the entry
/// is not a reparse point. Junctions and symbolic links have name-surrogate tags; a OneDrive
/// placeholder or a deduplicated file has another.
pub fn reparse_tag(path: &Path) -> io::Result<Option<u32>> {
    let w = wide(path)?;
    // SAFETY: an all-zero WIN32_FIND_DATAW is a valid value (integers and arrays of integers),
    // and it is only read after the call has filled it in.
    let mut data: WIN32_FIND_DATAW = unsafe { zeroed() };
    // SAFETY: `w` is NUL-terminated and outlives the call; `data` is a live, writable
    // WIN32_FIND_DATAW.
    let h = unsafe { FindFirstFileW(w.as_ptr(), &mut data) };
    if h == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `h` is the valid search handle the call above returned, closed exactly once.
    unsafe { FindClose(h) };
    Ok((data.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0).then_some(data.dwReserved0))
}

// --------------------------------------------------------------- the rename

/// `MoveFileExW` with `MOVEFILE_WRITE_THROUGH`: the call returns when the move is on disk, which
/// is what makes a commit rename a commit. `replace` adds `MOVEFILE_REPLACE_EXISTING` (files
/// only: the system refuses to replace a directory). Never copies across volumes.
pub fn move_file_write_through(from: &Path, to: &Path, replace: bool) -> io::Result<()> {
    let (f, t) = (wide(from)?, wide(to)?);
    let flags = MOVEFILE_WRITE_THROUGH
        | if replace {
            MOVEFILE_REPLACE_EXISTING
        } else {
            0
        };
    // SAFETY: both strings are NUL-terminated and outlive the call.
    let ok = unsafe { MoveFileExW(f.as_ptr(), t.as_ptr(), flags) };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

// ------------------------------------------------------ owner and access control

/// A `GetTokenInformation` result, kept in memory aligned for the structures it holds.
struct TokenInfo(Vec<u64>);

impl TokenInfo {
    fn of(class: i32) -> io::Result<TokenInfo> {
        let mut token: HANDLE = null_mut();
        // SAFETY: the pseudo handle of the current process needs no closing; `token` is a live,
        // writable HANDLE.
        let ok = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        let r = Self::query(token, class);
        // SAFETY: `token` is the valid handle opened above, closed exactly once.
        unsafe { CloseHandle(token) };
        r
    }

    fn query(token: HANDLE, class: i32) -> io::Result<TokenInfo> {
        let mut len = 0u32;
        // SAFETY: a null buffer of length 0 asks for the size needed, written to `len`.
        unsafe { GetTokenInformation(token, class, null_mut(), 0, &mut len) };
        if len == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut buf = vec![0u64; (len as usize).div_ceil(8)];
        // SAFETY: `buf` holds at least `len` writable bytes, 8-byte aligned, and `len` is the
        // size the system asked for.
        let ok = unsafe {
            GetTokenInformation(
                token,
                class,
                buf.as_mut_ptr().cast::<c_void>(),
                len,
                &mut len,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(TokenInfo(buf))
    }

    /// The SID of a `TokenUser` result. Valid while `self` lives.
    fn user_sid(&self) -> PSID {
        // SAFETY: the buffer was filled by GetTokenInformation(TokenUser) and is 8-byte
        // aligned, so it holds a TOKEN_USER.
        unsafe { (*self.0.as_ptr().cast::<TOKEN_USER>()).User.Sid }
    }

    /// The SID of a `TokenOwner` result. Valid while `self` lives.
    fn owner_sid(&self) -> PSID {
        // SAFETY: the buffer was filled by GetTokenInformation(TokenOwner) and is 8-byte
        // aligned, so it holds a TOKEN_OWNER.
        unsafe { (*self.0.as_ptr().cast::<TOKEN_OWNER>()).Owner }
    }
}

/// The owner of a file, and the security descriptor that owns the memory behind it.
struct Owner {
    sd: PSECURITY_DESCRIPTOR,
    sid: PSID,
}

impl Owner {
    fn of(path: &Path) -> io::Result<Owner> {
        let w = wide(path)?;
        let mut sid: PSID = null_mut();
        let mut sd: PSECURITY_DESCRIPTOR = null_mut();
        // SAFETY: `w` is NUL-terminated; `sid` and `sd` are live, writable out pointers; the
        // other out parameters are not requested and are null.
        let rc = unsafe {
            GetNamedSecurityInfoW(
                w.as_ptr(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION,
                &mut sid,
                null_mut(),
                null_mut(),
                null_mut(),
                &mut sd,
            )
        };
        if rc != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(rc as i32));
        }
        Ok(Owner { sd, sid })
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        // SAFETY: `sd` was allocated by GetNamedSecurityInfoW for the caller to free with
        // LocalFree, and is freed once, here.
        unsafe { LocalFree(self.sd) };
    }
}

/// Whether the owner of `path` is the user of this process's token. Also accepted, as `root` is
/// on Unix: the token's default owner (an elevated administrator's files are owned by the
/// Administrators group), the Administrators group, and the system account.
pub fn owner_is_current_user(path: &Path) -> io::Result<bool> {
    let owner = Owner::of(path)?;
    let user = TokenInfo::of(TokenUser)?;
    let default_owner = TokenInfo::of(TokenOwner)?;
    // SAFETY: both SIDs are valid for the whole function: `owner` and the token buffers live
    // until it returns.
    let same = |other: PSID| unsafe { EqualSid(owner.sid, other) != 0 };
    // SAFETY: `owner.sid` is a valid SID for as long as `owner` lives.
    let well_known = |kind: i32| unsafe { IsWellKnownSid(owner.sid, kind) != 0 };
    Ok(same(user.user_sid())
        || same(default_owner.owner_sid())
        || well_known(WinBuiltinAdministratorsSid)
        || well_known(WinLocalSystemSid))
}

/// Replace the access list of the directory at `path` with one that lets only the current user
/// in (full control, inherited by everything made inside later), and stop it inheriting from its
/// parent. This is what 0700 means on Windows. Existing children that inherit are updated by the
/// system, so call it on a directory the installer has just made.
pub fn protect_dir_current_user(path: &Path) -> io::Result<()> {
    let w = wide(path)?;
    let user = TokenInfo::of(TokenUser)?;
    // SAFETY: an all-zero EXPLICIT_ACCESS_W is a valid value (integers, a null pointer and an
    // enum-like integer in the trustee); every field the call reads is set below.
    let mut access: EXPLICIT_ACCESS_W = unsafe { zeroed() };
    access.grfAccessPermissions = FILE_ALL_ACCESS;
    access.grfAccessMode = SET_ACCESS;
    access.grfInheritance = SUB_CONTAINERS_AND_OBJECTS_INHERIT;
    access.Trustee = TRUSTEE_W {
        pMultipleTrustee: null_mut(),
        MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
        TrusteeForm: TRUSTEE_IS_SID,
        TrusteeType: TRUSTEE_IS_USER,
        // with TRUSTEE_IS_SID the name pointer carries the SID
        ptstrName: user.user_sid().cast::<u16>(),
    };
    let mut acl = null_mut();
    // SAFETY: `access` is one valid entry and the count says so; `acl` is a live out pointer
    // that receives a LocalAlloc'ed list; the SID in the trustee outlives the call.
    let rc = unsafe { SetEntriesInAclW(1, &access, null(), &mut acl) };
    if rc != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(rc as i32));
    }
    // SAFETY: `w` is NUL-terminated and, as the API wants a mutable pointer it never writes
    // through, is a live Vec; `acl` is the valid list from above; owner, group and SACL are
    // not set and are null.
    let rc = unsafe {
        SetNamedSecurityInfoW(
            w.as_ptr().cast_mut(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            acl,
            null(),
        )
    };
    // SAFETY: `acl` was allocated by SetEntriesInAclW for the caller to free with LocalFree.
    unsafe { LocalFree(acl.cast::<c_void>()) };
    if rc != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(rc as i32));
    }
    Ok(())
}
