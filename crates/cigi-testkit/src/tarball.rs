// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! Deterministic tar.gz building in memory (mtime 0, uid/gid 0, sorted by the caller), and
//! deliberately hostile archives for `unpack_tar_gz` tests.

use crate::must;
use flate2::write::GzEncoder;
use flate2::Compression;
use std::io::Write;

/// An in-memory archive under construction.
pub struct TarGz {
    builder: tar::Builder<Vec<u8>>,
}

impl Default for TarGz {
    fn default() -> Self {
        Self::new()
    }
}

fn header(size: u64, mode: u32, kind: tar::EntryType) -> tar::Header {
    let mut h = tar::Header::new_gnu();
    h.set_size(size);
    h.set_mode(mode);
    h.set_mtime(0);
    h.set_uid(0);
    h.set_gid(0);
    h.set_entry_type(kind);
    h
}

impl TarGz {
    pub fn new() -> TarGz {
        TarGz {
            builder: tar::Builder::new(Vec::new()),
        }
    }
    pub fn file(&mut self, path: &str, data: &[u8], exec: bool) -> &mut Self {
        let mut h = header(
            data.len() as u64,
            if exec { 0o755 } else { 0o644 },
            tar::EntryType::Regular,
        );
        must(self.builder.append_data(&mut h, path, data), "tar file");
        self
    }
    pub fn dir(&mut self, path: &str) -> &mut Self {
        let mut h = header(0, 0o755, tar::EntryType::Directory);
        must(
            self.builder.append_data(&mut h, path, std::io::empty()),
            "tar dir",
        );
        self
    }
    pub fn symlink(&mut self, path: &str, target: &str) -> &mut Self {
        let mut h = header(0, 0o777, tar::EntryType::Symlink);
        must(h.set_link_name(target), "tar link name");
        must(
            self.builder.append_data(&mut h, path, std::io::empty()),
            "tar symlink",
        );
        self
    }
    pub fn hardlink(&mut self, path: &str, target: &str) -> &mut Self {
        let mut h = header(0, 0o644, tar::EntryType::Link);
        must(h.set_link_name(target), "tar link name");
        must(
            self.builder.append_data(&mut h, path, std::io::empty()),
            "tar hardlink",
        );
        self
    }
    pub fn special(&mut self, path: &str, kind: tar::EntryType) -> &mut Self {
        let mut h = header(0, 0o666, kind);
        must(
            self.builder.append_data(&mut h, path, std::io::empty()),
            "tar special",
        );
        self
    }
    /// A regular file whose name is written byte for byte, bypassing the library's refusal of `..`
    /// and absolute paths. `declared` overrides the size field (the data may be shorter).
    pub fn raw_file(&mut self, name: &str, data: &[u8], declared: Option<u64>) -> &mut Self {
        let mut h = header(
            declared.unwrap_or(data.len() as u64),
            0o644,
            tar::EntryType::Regular,
        );
        {
            let old = h.as_old_mut();
            let n = name.as_bytes();
            old.name = [0u8; 100];
            old.name[..n.len().min(100)].copy_from_slice(&n[..n.len().min(100)]);
        }
        h.set_cksum();
        must(self.builder.append(&h, data), "tar raw file");
        self
    }
    /// Finish: the gzip of the tar, deterministic.
    pub fn finish(self) -> Vec<u8> {
        let tar = must(self.builder.into_inner(), "tar finish");
        let mut gz = GzEncoder::new(Vec::new(), Compression::fast());
        must(gz.write_all(&tar), "gzip");
        must(gz.finish(), "gzip finish")
    }
}

/// Hostile archives. Each returns the bytes of a `.tar.gz`.
pub mod evil {
    use super::TarGz;

    /// A normal file first, then `../escape`.
    pub fn zip_slip() -> Vec<u8> {
        let mut t = TarGz::new();
        t.file("top/ok.txt", b"fine", false)
            .raw_file("top/../../escape.txt", b"owned", None);
        t.finish()
    }
    pub fn dotdot_first() -> Vec<u8> {
        let mut t = TarGz::new();
        t.raw_file("../escape.txt", b"owned", None);
        t.finish()
    }
    pub fn absolute() -> Vec<u8> {
        let mut t = TarGz::new();
        t.raw_file("/tmp/cigi-absolute.txt", b"owned", None);
        t.finish()
    }
    pub fn symlink_out() -> Vec<u8> {
        let mut t = TarGz::new();
        t.symlink("top/link", "../../../etc/passwd");
        t.finish()
    }
    pub fn symlink_in() -> Vec<u8> {
        let mut t = TarGz::new();
        t.file("top/real", b"x", false).symlink("top/link", "real");
        t.finish()
    }
    pub fn hardlink() -> Vec<u8> {
        let mut t = TarGz::new();
        t.file("top/real", b"x", false)
            .hardlink("top/hard", "top/real");
        t.finish()
    }
    pub fn device() -> Vec<u8> {
        let mut t = TarGz::new();
        t.special("top/null", tar::EntryType::Char);
        t.finish()
    }
    pub fn fifo() -> Vec<u8> {
        let mut t = TarGz::new();
        t.special("top/pipe", tar::EntryType::Fifo);
        t.finish()
    }
    /// `n` empty files.
    pub fn many_entries(n: usize) -> Vec<u8> {
        let mut t = TarGz::new();
        for i in 0..n {
            t.file(&format!("top/f{i}"), b"", false);
        }
        t.finish()
    }
    /// A file whose header claims `claimed` bytes (the data is short: refusal happens on the header).
    pub fn huge_declared(claimed: u64) -> Vec<u8> {
        let mut t = TarGz::new();
        t.raw_file("top/big.bin", b"short", Some(claimed));
        t.finish()
    }
    pub fn duplicate() -> Vec<u8> {
        let mut t = TarGz::new();
        t.file("top/a", b"1", false).file("top/a", b"2", false);
        t.finish()
    }
    pub fn file_then_dir() -> Vec<u8> {
        let mut t = TarGz::new();
        t.file("top/a", b"1", false).file("top/a/b", b"2", false);
        t.finish()
    }
    pub fn backslash() -> Vec<u8> {
        let mut t = TarGz::new();
        t.raw_file("top/a\\b", b"1", None);
        t.finish()
    }
    /// Not a gzip at all.
    pub fn not_gzip() -> Vec<u8> {
        b"this is not an archive".to_vec()
    }
}
