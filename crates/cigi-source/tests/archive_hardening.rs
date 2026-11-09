// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0
//! Regression tests from the SEC-004 review of PR #36: unbounded tar extension headers and the
//! pax `size` / `path` differential.

use cigi_source::unpack_tar_gz;
use flate2::{write::GzEncoder, Compression};
use std::io::Write;

fn hdr(name: &str, ty: tar::EntryType, size: u64) -> tar::Header {
    let mut h = tar::Header::new_gnu();
    h.set_path(name).unwrap();
    h.set_entry_type(ty);
    h.set_size(size);
    h.set_mode(0o644);
    h.set_cksum();
    h
}

fn unpack(tar: &[u8]) -> (tempfile::TempDir, Result<usize, cigi_source::ArchiveError>) {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("a.tgz");
    let mut e = GzEncoder::new(std::fs::File::create(&f).unwrap(), Compression::fast());
    e.write_all(tar).unwrap();
    e.finish().unwrap();
    let r = unpack_tar_gz(&f, 0, &d.path().join("out")).map(|v| v.len());
    (d, r)
}

fn pax(records: &str) -> Vec<u8> {
    let mut t = Vec::new();
    t.extend_from_slice(hdr("pax", tar::EntryType::XHeader, records.len() as u64).as_bytes());
    t.extend_from_slice(records.as_bytes());
    t.resize(t.len().div_ceil(512) * 512, 0);
    t
}

fn record(key: &str, val: &str) -> String {
    let body = format!(" {key}={val}\n");
    let mut n = body.len() + 1;
    if (n.to_string().len() + body.len()) != n {
        n = body.len() + n.to_string().len();
    }
    format!("{n}{body}")
}

fn file(name: &str, size: u64, body: &[u8]) -> Vec<u8> {
    let mut t = hdr(name, tar::EntryType::Regular, size).as_bytes().to_vec();
    let mut b = body.to_vec();
    b.resize(b.len().div_ceil(512) * 512, 0);
    t.extend_from_slice(&b);
    t
}

#[test]
fn pax_size_override_is_refused() {
    let mut t = pax(&record("size", "5"));
    t.extend(file("f.txt", 0, b"hello"));
    t.extend_from_slice(&[0u8; 1024]);
    let (d, r) = unpack(&t);
    let e = r.unwrap_err();
    assert_eq!(e.code(), "E1826", "{e}");
    assert!(!d.path().join("out/f.txt").exists());
}

#[test]
fn pax_path_override_is_refused() {
    let mut t = pax(&record("path", "other/name"));
    t.extend(file("f.txt", 2, b"hi"));
    t.extend_from_slice(&[0u8; 1024]);
    let (_d, r) = unpack(&t);
    assert_eq!(r.unwrap_err().code(), "E1826");
}

#[test]
fn harmless_pax_records_are_still_accepted() {
    let mut t = pax(&record("comment", "built by a person"));
    t.extend(file("f.txt", 2, b"hi"));
    t.extend_from_slice(&[0u8; 1024]);
    let (_d, r) = unpack(&t);
    assert_eq!(r.unwrap(), 1);
}

#[test]
fn oversize_long_name_header_is_refused_before_it_is_read_into_memory() {
    // 64 KiB + 1 of name: the tar reader would have read the whole record into a Vec
    let n = 64 * 1024 + 1;
    let mut t = hdr("././@LongLink", tar::EntryType::GNULongName, n as u64)
        .as_bytes()
        .to_vec();
    let mut name = vec![b'a'; n];
    name.resize(n.div_ceil(512) * 512, 0);
    t.extend_from_slice(&name);
    t.extend(file("x", 0, b""));
    t.extend_from_slice(&[0u8; 1024]);
    let (_d, r) = unpack(&t);
    let e = r.unwrap_err();
    assert_eq!(e.code(), "E1826", "{e}");
    assert!(e.to_string().contains("64 KiB"), "{e}");
}

#[test]
fn huge_declared_long_name_does_not_allocate() {
    // a header that claims 3 GiB of long name and has no data: refused from the header alone
    let mut t = hdr("././@LongLink", tar::EntryType::GNULongLink, 3 << 30)
        .as_bytes()
        .to_vec();
    t.extend_from_slice(&[0u8; 1024]);
    let (_d, r) = unpack(&t);
    assert_eq!(r.unwrap_err().code(), "E1826");
}

#[test]
fn a_normal_long_name_still_extracts() {
    let name = format!("dir/{}", "n".repeat(150));
    let mut t = hdr(
        "././@LongLink",
        tar::EntryType::GNULongName,
        name.len() as u64 + 1,
    )
    .as_bytes()
    .to_vec();
    let mut data = name.clone().into_bytes();
    data.push(0);
    data.resize(512, 0);
    t.extend_from_slice(&data);
    t.extend(file("short", 2, b"ok"));
    t.extend_from_slice(&[0u8; 1024]);
    let (d, r) = unpack(&t);
    assert_eq!(r.unwrap(), 1);
    assert!(d.path().join("out").join(&name).is_file());
}
