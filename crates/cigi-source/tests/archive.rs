// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0
//! `unpack_tar_gz`: the refusals of section 14.2 and the happy path.

use cigi_source::{unpack_tar_gz, ArchiveError};
use cigi_testkit::{evil, TarGz};

fn unpack(
    bytes: &[u8],
    strip: u32,
) -> (
    tempfile::TempDir,
    Result<Vec<cigi_home::StagedFile>, ArchiveError>,
) {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("a.tar.gz");
    std::fs::write(&f, bytes).unwrap();
    let r = unpack_tar_gz(&f, strip, &d.path().join("out"));
    (d, r)
}

fn refused(bytes: &[u8], strip: u32) -> ArchiveError {
    let (d, r) = unpack(bytes, strip);
    let e = r.unwrap_err();
    assert_eq!(e.code(), "E1826", "{e}");
    // nothing escaped
    assert!(!d.path().join("escape.txt").exists());
    assert!(!d.path().parent().unwrap().join("escape.txt").exists());
    e
}

#[test]
fn good_archive_with_strip() {
    let mut t = TarGz::new();
    t.dir("top/")
        .file("top/bin/tool", b"#!/bin/sh\n", true)
        .file("top/readme", b"hi", false)
        .dir("top/empty/");
    let (d, r) = unpack(&t.finish(), 1);
    let files = r.unwrap();
    let names: Vec<_> = files
        .iter()
        .map(|f| f.path.to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["bin/tool", "readme"]);
    assert!(files[0].exec && !files[1].exec);
    let staged = std::fs::read(&files[1].source).unwrap();
    assert_eq!(staged.len(), 2);
    assert_eq!(
        cigi_testkit::sha256_hex(&staged),
        cigi_testkit::sha256_hex(b"hi")
    );
    assert!(d.path().join("out/bin/tool").is_file());
    assert!(d.path().join("out/empty").is_dir());
}

#[test]
fn strip_zero_keeps_everything() {
    let mut t = TarGz::new();
    t.file("a", b"1", false).file("d/b", b"2", false);
    let (_d, r) = unpack(&t.finish(), 0);
    assert_eq!(r.unwrap().len(), 2);
}

#[test]
fn a_file_outside_the_stripped_directory_is_refused() {
    let mut t = TarGz::new();
    t.file("loose", b"1", false);
    refused(&t.finish(), 1);
}

#[test]
fn zip_slip_and_absolute() {
    for b in [evil::zip_slip(), evil::dotdot_first(), evil::absolute()] {
        let e = refused(&b, 0);
        assert!(matches!(e, ArchiveError::Unsafe { .. }));
    }
    // also when strip would swallow the traversal's first component
    refused(&evil::zip_slip(), 1);
}

#[test]
fn links_devices_and_fifos() {
    for b in [
        evil::symlink_out(),
        evil::symlink_in(),
        evil::hardlink(),
        evil::device(),
        evil::fifo(),
    ] {
        refused(&b, 0);
    }
}

#[test]
fn too_many_entries() {
    let e = refused(&evil::many_entries(20_001), 0);
    assert!(matches!(e, ArchiveError::TooBig { .. }));
    // exactly at the limit is fine
    let (_d, r) = unpack(&evil::many_entries(20_000), 0);
    assert_eq!(r.unwrap().len(), 20_000);
}

#[test]
fn file_over_256_mb() {
    let e = refused(&evil::huge_declared(256 * 1024 * 1024 + 1), 0);
    assert!(matches!(e, ArchiveError::TooBig { .. }));
}

#[test]
fn duplicates_and_file_directory_clashes() {
    refused(&evil::duplicate(), 0);
    refused(&evil::file_then_dir(), 0);
}

#[test]
fn backslashes_are_refused() {
    refused(&evil::backslash(), 0);
}

#[test]
fn not_a_gzip_is_unreadable_not_a_panic() {
    let e = refused(&evil::not_gzip(), 0);
    assert!(matches!(e, ArchiveError::Unreadable(_)));
}

#[test]
fn a_cut_off_archive_is_unreadable() {
    let mut t = TarGz::new();
    t.file("top/a", &vec![7u8; 100_000], false);
    let b = t.finish();
    let (_d, r) = unpack(&b[..b.len() / 2], 0);
    let e = r.unwrap_err();
    assert_eq!(e.code(), "E1826");
}
