// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! The only place in this crate where `cfg(unix)` / `cfg(windows)` appear.

use std::borrow::Cow;
use std::fs;
use std::io;
use std::path::Path;
use std::time::Duration;

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

    pub fn file_id(path: &Path) -> Option<super::FileId> {
        fs::metadata(path).ok().map(|m| super::FileId {
            volume: m.dev(),
            index: u128::from(m.ino()),
        })
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

    #[cfg(windows)]
    pub fn file_id(path: &Path) -> Option<super::FileId> {
        let i = cigi_sys::fs::file_info(&super::long_path(path)).ok()?;
        Some(super::FileId {
            volume: u64::from(i.volume),
            index: u128::from(i.index),
        })
    }

    #[cfg(not(windows))]
    pub fn file_id(_path: &Path) -> Option<super::FileId> {
        None
    }

    pub fn ino(path: &Path) -> Option<u64> {
        file_id(path).map(|id| super::fold_id(id.volume, id.index))
    }

    #[cfg(windows)]
    pub fn nlink(path: &Path) -> Option<u64> {
        let i = cigi_sys::fs::file_info(&super::long_path(path)).ok()?;
        Some(u64::from(i.links))
    }

    #[cfg(not(windows))]
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
        if super::is_link_md(&md, path) {
            return Err("a symbolic link".to_string());
        }
        if !md.is_dir() {
            return Err("not a directory".to_string());
        }
        owner_ok(path)
    }

    /// The owner is the user of this process (or the administrators or the system, as root is
    /// on Unix). Where the inherited DACL under the user's profile applies, that is enough
    /// besides: nobody else gets in by inheritance.
    #[cfg(windows)]
    fn owner_ok(path: &Path) -> Result<(), String> {
        match cigi_sys::fs::owner_is_current_user(&super::long_path(path)) {
            Ok(true) => Ok(()),
            Ok(false) => Err("owned by another user".to_string()),
            Err(e) => Err(format!("cannot read the owner: {e}")),
        }
    }

    #[cfg(not(windows))]
    fn owner_ok(_path: &Path) -> Result<(), String> {
        Ok(())
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

// ------------------------------------------------------------ file identity

/// Which file a path names: the volume and the file's number on it. Two paths with the same
/// `FileId` are one file (hard links of each other, or the same path). `None` from [`file_id`]
/// means the platform cannot say, never "different".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileId {
    volume: u64,
    index: u128,
}

/// The identity of the file at `path`, links followed: device and inode on Unix, volume serial
/// number and file index from `GetFileInformationByHandle` on Windows. Never compare the results
/// of `canonicalize` for this: on Windows they differ by `\\?\` prefix, case and short names.
pub fn file_id(path: &Path) -> Option<FileId> {
    imp::file_id(path)
}

/// Both paths name one file. `false` when either identity is unknown.
pub fn same_file(a: &Path, b: &Path) -> bool {
    matches!((file_id(a), file_id(b)), (Some(x), Some(y)) if x == y)
}

/// A 64-bit digest of a volume and a file index: what [`ino`] returns on Windows, where there is
/// no single 64-bit number. Callers that must be exact use [`file_id`].
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn fold_id(volume: u64, index: u128) -> u64 {
    fn mix(mut x: u64) -> u64 {
        x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        x ^ (x >> 31)
    }
    mix(mix(volume) ^ mix((index >> 64) as u64) ^ mix(index as u64).rotate_left(17))
}

// ---------------------------------------------------- retries and attributes

/// Windows errors worth another try: `ERROR_ACCESS_DENIED` (5), `ERROR_SHARING_VIOLATION` (32) and
/// `ERROR_LOCK_VIOLATION` (33). A virus scanner, the search indexer or a program that has just
/// exited holds a file for a moment; nothing else is retried.
pub fn is_sharing_error(e: &io::Error) -> bool {
    cfg!(windows) && matches!(e.raw_os_error(), Some(5 | 32 | 33))
}

/// The waits between tries, in milliseconds: about three seconds in all.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) const RETRY_DELAYS_MS: [u64; 9] = [10, 20, 40, 80, 160, 320, 640, 800, 900];

/// Run `op`; while it fails with an error `retryable` accepts, wait (`sleep`) and run it again,
/// following [`RETRY_DELAYS_MS`]. The last error is returned when the waits run out.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn retry_with<T>(
    mut op: impl FnMut() -> io::Result<T>,
    retryable: impl Fn(&io::Error) -> bool,
    mut sleep: impl FnMut(Duration),
) -> io::Result<T> {
    let mut waits = RETRY_DELAYS_MS.iter();
    loop {
        match op() {
            Err(e) if retryable(&e) => match waits.next() {
                Some(ms) => sleep(Duration::from_millis(*ms)),
                None => return Err(e),
            },
            r => return r,
        }
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
fn retry<T>(op: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    retry_with(op, is_sharing_error, std::thread::sleep)
}

/// What a failed directory rename should say on Windows: a program running out of the directory
/// keeps it from being renamed.
pub const CLOSE_RUNNING: &str = "close running cig programs and try again";

#[cfg_attr(not(windows), allow(dead_code))]
fn with_hint(e: io::Error, hint: &str) -> io::Error {
    io::Error::new(e.kind(), format!("{e} ({hint})"))
}

/// Set or clear the read-only attribute of a file (Windows only; elsewhere nothing). Links are
/// never followed to change something else. Best effort: the caller's next call reports the
/// real failure.
pub fn set_readonly(path: &Path, readonly: bool) {
    if cfg!(windows) {
        let Ok(md) = fs::symlink_metadata(path) else {
            return;
        };
        if !md.is_file() || md.permissions().readonly() == readonly {
            return;
        }
        set_readonly_attr(path, md.permissions(), readonly);
    }
}

/// The attribute is all `set_readonly` changes on Windows, the only place this is called from
/// (it is a no-op guard elsewhere).
fn set_readonly_attr(path: &Path, mut perms: fs::Permissions, readonly: bool) {
    perms.set_readonly(readonly);
    let _ = fs::set_permissions(path, perms);
}

/// Clear the read-only attribute on every file and directory under `dir` (never through a link),
/// and on `dir`, so the tree can be removed. Windows only; a toolchain's files are read-only
/// (SPEC 4.3).
pub fn clear_readonly_tree(dir: &Path) {
    if !cfg!(windows) {
        return;
    }
    let Ok(md) = fs::symlink_metadata(dir) else {
        return;
    };
    if is_link_md(&md, dir) {
        return;
    }
    if md.permissions().readonly() {
        set_readonly_attr(dir, md.permissions(), false);
    }
    if md.is_dir() {
        if let Ok(rd) = fs::read_dir(dir) {
            for e in rd.flatten() {
                clear_readonly_tree(&e.path());
            }
        }
    }
}

/// `rename`, as the journal needs it. On Windows: the read-only attribute of a file about to be
/// replaced is cleared, a sharing or access error is retried with a backoff of about three
/// seconds, and the move is `MoveFileExW` with write-through so it is on disk when the call
/// returns. A directory that stays busy gets the hint to close running cig programs.
pub fn rename(from: &Path, to: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        win::rename(from, to)
    }
    #[cfg(not(windows))]
    {
        fs::rename(from, to)
    }
}

/// `remove_file`, retried on Windows sharing errors, with the read-only attribute cleared first.
/// A directory link (a junction or a directory symbolic link) is removed as the link it is.
pub fn remove_file(path: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        win::remove_file(path)
    }
    #[cfg(not(windows))]
    {
        fs::remove_file(path)
    }
}

/// `remove_dir_all`, retried on Windows sharing errors; when it still fails, the read-only
/// attributes under it are cleared and it is tried again.
pub fn remove_dir_all(path: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        win::remove_dir_all(path)
    }
    #[cfg(not(windows))]
    {
        fs::remove_dir_all(path)
    }
}

/// Remove a file, link or directory tree; absent is fine. Links are removed, not followed.
pub fn remove_any(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(md) if md.is_dir() && !is_link_md(&md, path) => remove_dir_all(path),
        Ok(_) => remove_file(path),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Flush a file already on disk. Windows needs write access to flush, and a store object is
/// read-only: the attribute is lifted for the flush and put back.
pub fn sync_file(path: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        win::sync_file(path)
    }
    #[cfg(not(windows))]
    {
        fs::File::open(path)?.sync_all()
    }
}

/// Whether published store objects are flushed in a batch at the next barrier. Windows flushes
/// each object while its writer handle is still open, before the read-only attribute goes on.
pub const BATCHED_FSYNC: bool = cfg!(not(windows));

// ----------------------------------------------------------- paths and names

/// The path to hand to a call that does not normalize it. On Windows a path of 240 characters or
/// more becomes `\\?\`-prefixed, with separators and `.` and `..` already resolved (the prefix
/// switches the system's own normalizing off). Elsewhere, and for a short path, unchanged.
pub fn long_path(path: &Path) -> Cow<'_, Path> {
    if cfg!(windows) {
        if let Some(s) = path.to_str() {
            if s.chars().count() >= 240 {
                if let Some(v) = to_verbatim(s) {
                    return Cow::Owned(v.into());
                }
            }
        }
    }
    Cow::Borrowed(path)
}

/// `C:\a\b` becomes `\\?\C:\a\b` and `\\srv\share\a` becomes `\\?\UNC\srv\share\a`, with `/` turned
/// into `\` and `.` and `..` resolved. `None` for a path that is already verbatim or device, and
/// for one that is not absolute with a drive or a share.
pub(crate) fn to_verbatim(p: &str) -> Option<String> {
    if p.starts_with("\\\\?\\") || p.starts_with("\\\\.\\") {
        return None;
    }
    let s = p.replace('/', "\\");
    let b = s.as_bytes();
    let (prefix, rest, floor) = if let Some(unc) = s.strip_prefix("\\\\") {
        ("\\\\?\\UNC\\".to_string(), unc.to_string(), 2)
    } else if b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\' {
        (format!("\\\\?\\{}:\\", b[0] as char), s[3..].to_string(), 0)
    } else {
        return None;
    };
    let mut parts: Vec<&str> = Vec::new();
    for c in rest.split('\\') {
        match c {
            "" | "." => {}
            ".." => {
                if parts.len() > floor {
                    parts.pop();
                }
            }
            c => parts.push(c),
        }
    }
    Some(format!("{prefix}{}", parts.join("\\")))
}

/// `\\?\C:\a` and `\\?\UNC\srv\sh\a` back to `C:\a` and `\\srv\sh\a`.
fn strip_verbatim(p: &str) -> String {
    if let Some(r) = p.strip_prefix("\\\\?\\UNC\\") {
        format!("\\\\{r}")
    } else if let Some(r) = p.strip_prefix("\\\\?\\") {
        r.to_string()
    } else {
        p.to_string()
    }
}

fn fold_components(p: &str) -> Vec<String> {
    let p = strip_verbatim(p);
    let mut out: Vec<String> = Vec::new();
    for c in p.split(['\\', '/']) {
        match c {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            c => out.push(c.to_lowercase()),
        }
    }
    out
}

/// `starts_with` as Windows means it: by whole components, ignoring case, both separators, a
/// `\\?\` prefix and `.` parts. (Short 8.3 names are not resolved.)
pub(crate) fn windows_starts_with(path: &str, base: &str) -> bool {
    let (a, b) = (fold_components(path), fold_components(base));
    a.len() >= b.len() && a[..b.len()] == b[..]
}

/// Whether `path` lies at or under `base`: `Path::starts_with` everywhere but Windows, where the
/// comparison ignores case and the `\\?\` prefix. Containment checks use this, never a
/// comparison of `canonicalize` results.
pub fn starts_with(path: &Path, base: &Path) -> bool {
    if cfg!(windows) {
        windows_starts_with(&path.to_string_lossy(), &base.to_string_lossy())
    } else {
        path.starts_with(base)
    }
}

/// Both paths are the same place (by [`starts_with`] both ways).
pub fn same_path(a: &Path, b: &Path) -> bool {
    starts_with(a, b) && starts_with(b, a)
}

/// `path` lies under the user's profile (`%USERPROFILE%`): there the inherited access list is
/// enough, and nobody else is let in by it.
pub fn is_under_user_profile(path: &Path) -> bool {
    under_profile(
        std::env::var_os("USERPROFILE").as_deref().map(Path::new),
        path,
    )
}

pub(crate) fn under_profile(profile: Option<&Path>, path: &Path) -> bool {
    profile.is_some_and(|p| !p.as_os_str().is_empty() && starts_with(path, p))
}

/// A home path longer than this earns a warning on Windows.
pub const LONG_HOME_CHARS: usize = 100;

/// The warning for a home path over [`LONG_HOME_CHARS`] characters. The caller says it on
/// Windows only.
pub fn long_home_message(root: &Path) -> Option<String> {
    let n = root.to_string_lossy().chars().count();
    (n > LONG_HOME_CHARS).then(|| {
        format!(
            "the home path is {n} characters long; a deep toolchain path may pass the Windows path limit, so a shorter CIGSCRIPT_HOME is safer"
        )
    })
}

/// Why `name` cannot be one file or directory name on Windows, `None` when it can. Reserved
/// device names (`CON`, `NUL`, `COM1`, ... with or without an extension), a trailing dot or
/// space, a colon (an alternate data stream), the characters `< > " | ? *` and control
/// characters are all refused, so a manifest that installs on one system installs on the rest.
pub fn windows_name_problem(name: &str) -> Option<&'static str> {
    if name.is_empty() {
        return Some("is empty");
    }
    if name.contains(':') {
        return Some("contains a colon (Windows reads that as an alternate data stream)");
    }
    if name
        .chars()
        .any(|c| c.is_control() || matches!(c, '<' | '>' | '"' | '|' | '?' | '*'))
    {
        return Some("contains a character Windows does not allow in a name");
    }
    if name.ends_with('.') || name.ends_with(' ') {
        return Some("ends in a dot or a space (Windows drops it)");
    }
    let stem = name.split('.').next().unwrap_or(name).trim_end_matches(' ');
    let up = stem.to_ascii_uppercase();
    let reserved = matches!(
        up.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || ((up.starts_with("COM") || up.starts_with("LPT"))
        && up.len() == 4
        && up.as_bytes()[3].is_ascii_digit());
    reserved.then_some("is a reserved Windows device name")
}

// -------------------------------------------------------------------- links

/// Whether the entry the metadata describes is a link the installer must not follow. On Unix a
/// symbolic link. On Windows a reparse point whose tag says it names another path (a symbolic
/// link or a junction, found by `FILE_ATTRIBUTE_REPARSE_POINT` and the tag); a reparse point of
/// another kind, a OneDrive placeholder or a deduplicated file, is an ordinary file or
/// directory and is not refused.
pub fn is_link_md(md: &fs::Metadata, path: &Path) -> bool {
    #[cfg(windows)]
    {
        win::is_link_md(md, path)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        md.file_type().is_symlink()
    }
}

/// [`is_link_md`] for a path (not followed). A path that cannot be examined is not a link.
pub fn is_link(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|md| is_link_md(&md, path))
}

/// The tag bit that marks a reparse point as naming another path (`IsReparseTagNameSurrogate`):
/// set for symbolic links and junctions, clear for OneDrive, dedup and the rest.
pub fn reparse_tag_is_link(tag: u32) -> bool {
    tag & 0x2000_0000 != 0
}

// -------------------------------------------------------------- access control

/// Make a directory the installer has just made the user's alone. Under the user's profile the
/// inherited access list already is, and nothing is changed. Elsewhere on Windows the directory
/// gets an access list of the current user only, not inheriting from its parent. On Unix the
/// mode (0700) does this and the caller has set it.
pub fn restrict_to_owner(path: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        if is_under_user_profile(path) {
            return Ok(());
        }
        cigi_sys::fs::protect_dir_current_user(&long_path(path))
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Ok(())
    }
}

#[cfg(windows)]
mod win {
    use super::{
        clear_readonly_tree, is_sharing_error, long_path, reparse_tag_is_link, retry, set_readonly,
        with_hint, CLOSE_RUNNING,
    };
    use std::fs::{self, OpenOptions};
    use std::io;
    use std::os::windows::fs::MetadataExt;
    use std::path::Path;

    const DIRECTORY: u32 = 0x10;
    const REPARSE_POINT: u32 = 0x400;

    pub fn is_link_md(md: &fs::Metadata, path: &Path) -> bool {
        if md.file_attributes() & REPARSE_POINT == 0 {
            return false;
        }
        match cigi_sys::fs::reparse_tag(&long_path(path)) {
            Ok(Some(tag)) => reparse_tag_is_link(tag),
            // cannot tell: refuse what std says is a link, and nothing else
            _ => md.file_type().is_symlink(),
        }
    }

    pub fn rename(from: &Path, to: &Path) -> io::Result<()> {
        let (f, t) = (long_path(from), long_path(to));
        let is_dir = fs::symlink_metadata(&f).is_ok_and(|m| m.is_dir());
        if !is_dir {
            // a read-only file in the way of a replace is refused with access denied
            set_readonly(&t, false);
        }
        let res = retry(|| cigi_sys::fs::move_file_write_through(&f, &t, !is_dir));
        match res {
            Err(e) if is_dir && is_sharing_error(&e) => Err(with_hint(e, CLOSE_RUNNING)),
            r => r,
        }
    }

    pub fn remove_file(path: &Path) -> io::Result<()> {
        let p = long_path(path);
        let md = fs::symlink_metadata(&p)?;
        if md.file_attributes() & DIRECTORY != 0 {
            // a junction or a directory symbolic link: removed as the link it is
            return retry(|| fs::remove_dir(&p));
        }
        set_readonly(&p, false);
        retry(|| fs::remove_file(&p))
    }

    pub fn remove_dir_all(path: &Path) -> io::Result<()> {
        let p = long_path(path);
        match retry(|| fs::remove_dir_all(&p)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => {
                clear_readonly_tree(&p);
                retry(|| fs::remove_dir_all(&p)).map_err(|e2| {
                    if is_sharing_error(&e2) {
                        with_hint(e2, CLOSE_RUNNING)
                    } else {
                        e2
                    }
                })
            }
            r => r,
        }
    }

    pub fn sync_file(path: &Path) -> io::Result<()> {
        let p = long_path(path);
        let open = || OpenOptions::new().write(true).open(&p);
        match open() {
            Ok(f) => f.sync_all(),
            Err(e) if e.raw_os_error() == Some(5) => {
                // read-only attribute: lift it for the flush, put it back after
                set_readonly(&p, false);
                let r = open().and_then(|f| f.sync_all());
                set_readonly(&p, true);
                r
            }
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    #[test]
    fn retry_waits_through_the_schedule_then_gives_up() {
        let tries = Cell::new(0);
        let waited = RefCell::new(Vec::new());
        let r: io::Result<()> = retry_with(
            || {
                tries.set(tries.get() + 1);
                Err(io::Error::other("busy"))
            },
            |_| true,
            |d| waited.borrow_mut().push(d),
        );
        assert!(r.is_err());
        assert_eq!(tries.get(), RETRY_DELAYS_MS.len() + 1);
        let total: Duration = waited.borrow().iter().sum();
        assert!(total >= Duration::from_millis(2500) && total <= Duration::from_millis(3500));
    }

    #[test]
    fn retry_stops_as_soon_as_the_call_works_and_skips_other_errors() {
        let tries = Cell::new(0);
        let r = retry_with(
            || {
                tries.set(tries.get() + 1);
                if tries.get() < 3 {
                    Err(io::Error::other("busy"))
                } else {
                    Ok(7)
                }
            },
            |_| true,
            |_| {},
        );
        assert_eq!(r.unwrap(), 7);
        assert_eq!(tries.get(), 3);
        let n = Cell::new(0);
        let r: io::Result<()> = retry_with(
            || {
                n.set(n.get() + 1);
                Err(io::Error::from(io::ErrorKind::NotFound))
            },
            |e| e.kind() == io::ErrorKind::PermissionDenied,
            |_| panic!("no wait for an error that is not retried"),
        );
        assert!(r.is_err());
        assert_eq!(n.get(), 1);
    }

    #[test]
    fn only_errors_5_32_and_33_are_sharing_errors_and_only_on_windows() {
        for code in [5, 32, 33] {
            assert_eq!(
                is_sharing_error(&io::Error::from_raw_os_error(code)),
                cfg!(windows)
            );
        }
        assert!(!is_sharing_error(&io::Error::from_raw_os_error(2)));
        assert!(!is_sharing_error(&io::Error::from_raw_os_error(112)));
    }

    #[test]
    fn reserved_and_unportable_names_are_refused() {
        for bad in [
            "con",
            "CON",
            "Nul",
            "aux.txt",
            "prn.tar.gz",
            "com1",
            "LPT9.log",
            "com1 .txt",
            "a:b",
            "stream:$DATA",
            "x.",
            "x ",
            "a<b",
            "a|b",
            "a?b",
            "a*b",
            "a\"b",
            "tab\there",
            "CONIN$",
            "",
        ] {
            assert!(windows_name_problem(bad).is_some(), "{bad:?}");
        }
        for ok in [
            "cig",
            "console",
            "conf.toml",
            "com10",
            "com",
            "lpt",
            "auxiliary",
            "a.b.c",
            ".hidden",
            "orchestrate.cig",
            "my file",
            "cigo.exe",
            "nul-device",
            "COM1x",
        ] {
            assert!(windows_name_problem(ok).is_none(), "{ok:?}");
        }
    }

    #[test]
    fn containment_ignores_case_separators_and_the_verbatim_prefix() {
        assert!(windows_starts_with(
            r"C:\Users\Jay\.cigscript\bin",
            r"c:\users\JAY\.cigscript"
        ));
        assert!(windows_starts_with(
            r"C:\Users\Jay\.cigscript",
            r"C:\Users\Jay\.cigscript"
        ));
        assert!(windows_starts_with(r"\\?\C:\Users\Jay\x", r"C:\users\jay"));
        assert!(windows_starts_with("C:/Users/Jay/x", r"C:\Users\Jay\"));
        assert!(windows_starts_with(r"\\?\UNC\srv\share\x", r"\\SRV\Share"));
        assert!(windows_starts_with(r"C:\Users\.\Jay\x", r"C:\Users\Jay"));
        // whole components, not a string prefix
        assert!(!windows_starts_with(r"C:\Users\JayZ\x", r"C:\Users\Jay"));
        assert!(!windows_starts_with(r"D:\Users\Jay\x", r"C:\Users\Jay"));
        assert!(!windows_starts_with(r"C:\Users", r"C:\Users\Jay"));
        // a `..` cannot climb back out
        assert!(!windows_starts_with(
            r"C:\Users\Jay\..\Other",
            r"C:\Users\Jay"
        ));
    }

    #[test]
    fn under_the_profile_is_decided_by_that_containment() {
        assert!(!under_profile(None, Path::new("/x")));
        assert!(!under_profile(Some(Path::new("")), Path::new("/x")));
        assert!(under_profile(
            Some(Path::new("/h/jay")),
            Path::new("/h/jay/.cigscript")
        ));
        assert!(!under_profile(
            Some(Path::new("/h/jay")),
            Path::new("/srv/cig")
        ));
    }

    #[test]
    fn long_paths_get_the_verbatim_prefix_normalized() {
        assert_eq!(
            to_verbatim(r"C:\a\b\..\c\.\d").as_deref(),
            Some(r"\\?\C:\a\c\d")
        );
        assert_eq!(to_verbatim("C:/a/b").as_deref(), Some(r"\\?\C:\a\b"));
        assert_eq!(
            to_verbatim(r"\\srv\share\a\..\b").as_deref(),
            Some(r"\\?\UNC\srv\share\b")
        );
        // never climbs out of the share
        assert_eq!(
            to_verbatim(r"\\srv\share\..\..\x").as_deref(),
            Some(r"\\?\UNC\srv\share\x")
        );
        assert_eq!(to_verbatim(r"\\?\C:\a"), None);
        assert_eq!(to_verbatim("relative\\path"), None);
        assert_eq!(to_verbatim("/unix/path"), None);
    }

    #[test]
    fn a_home_path_over_100_characters_is_warned_about() {
        assert!(long_home_message(Path::new("/short")).is_none());
        let long = "x".repeat(LONG_HOME_CHARS + 1);
        assert!(long_home_message(Path::new(&long)).is_some());
        assert!(long_home_message(Path::new(&"x".repeat(LONG_HOME_CHARS))).is_none());
    }

    #[test]
    fn only_name_surrogate_reparse_tags_are_links() {
        // IO_REPARSE_TAG_SYMLINK, IO_REPARSE_TAG_MOUNT_POINT (a junction)
        assert!(reparse_tag_is_link(0xA000_000C));
        assert!(reparse_tag_is_link(0xA000_0003));
        // OneDrive (cloud files), dedup, WSL, app execution links: ordinary files
        for tag in [
            0x9000_001A_u32,
            0x8000_0013,
            0x8000_0017,
            0x8000_001B,
            0x9000_6000,
        ] {
            assert!(!reparse_tag_is_link(tag), "{tag:#x}");
        }
    }

    #[test]
    fn identities_are_exact_for_one_file_and_digested_apart() {
        let t = tempfile::tempdir().unwrap();
        let a = t.path().join("a");
        let b = t.path().join("b");
        let c = t.path().join("c");
        fs::write(&a, "x").unwrap();
        fs::write(&c, "x").unwrap();
        fs::hard_link(&a, &b).unwrap();
        assert!(same_file(&a, &b));
        assert!(!same_file(&a, &c));
        assert!(!same_file(&a, &t.path().join("missing")));
        assert_eq!(file_id(&a), file_id(&b));
        assert_eq!(ino(&a), ino(&b));
        assert_ne!(ino(&a), ino(&c));
        assert_eq!(nlink(&a), Some(2));
        assert_ne!(fold_id(1, 5), fold_id(2, 5));
        assert_ne!(fold_id(1, 5), fold_id(1, 6));
        assert_ne!(fold_id(1, 1 << 64), fold_id(1, 1));
    }

    #[test]
    fn the_plain_wrappers_do_what_std_does_here() {
        let t = tempfile::tempdir().unwrap();
        let (a, b) = (t.path().join("a"), t.path().join("b"));
        fs::write(&a, "1").unwrap();
        fs::write(&b, "2").unwrap();
        rename(&a, &b).unwrap();
        assert_eq!(fs::read_to_string(&b).unwrap(), "1");
        remove_file(&b).unwrap();
        assert!(!b.exists());
        let d = t.path().join("d/e");
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("f"), "x").unwrap();
        remove_any(&t.path().join("d")).unwrap();
        remove_any(&t.path().join("d")).unwrap();
        assert!(!t.path().join("d").exists());
        fs::write(&a, "y").unwrap();
        sync_file(&a).unwrap();
        assert!(!is_link(&a));
    }
}
