// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! A compiled program, read from its trailer (#30). A program is a CigScript script appended to the
//! `cig` of one version; its file ends in:
//!
//! ```text
//! ... cig ...                                 the runtime, as released
//! payload                                     len bytes of JSON
//! "CIGPROG1"  len (u64, LE)  sha256(payload)  the trailer, 48 bytes
//! ```
//!
//! The layout is the one cig documents (`docs/PROGRAMS.md` in cig-script); this reader depends on none
//! of cig's crates. It never runs a program and never trusts one: every length is bounded before it is
//! read, and anything that does not match is an `Err` that says what, never a panic.
//!
//! Not read here: a program whose trailer is gone (cig finds it through its runtime's image headers; it
//! cannot be named without them), and a Windows program signed after it was built (a certificate table
//! follows its trailer; cig-compile#10).

use std::io::{Read, Seek, SeekFrom};

use sha2::{Digest, Sha256};

use crate::{Advisory, Manifest, Version};

/// The trailer's first eight bytes.
pub const MAGIC: [u8; 8] = *b"CIGPROG1";
/// The trailer's length: magic, payload length, payload sha256.
pub const TRAILER_LEN: u64 = 48;
/// The payload's format.
pub const FORMAT: u64 = 1;
/// The most a payload may hold.
pub const MAX_PAYLOAD: u64 = 16 * 1024 * 1024;
/// How many zero bytes may follow a trailer and leave the program whole.
pub const PAD_MAX: u64 = 4096;

/// What a program's record says about it. The script itself is checked and then dropped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Program {
    pub name: String,
    /// The script's path as it was compiled.
    pub file: String,
    /// sha256 of the script, hex, as the record says (checked against the script it carries).
    pub sha256: String,
    /// The cig version it was compiled for and runs on.
    pub cig: String,
    /// What compiled it, e.g. `cigcompile 0.2.0`.
    pub compiler: String,
    /// The embedded script's size in bytes.
    pub source_bytes: usize,
}

impl Program {
    /// The cig version as a [`Version`], when it parses.
    pub fn cig_version(&self) -> Option<Version> {
        Version::parse(&self.cig).ok()
    }
}

/// The program at the end of `f`. `Ok(None)`: no trailer is there (not a program, or one this reader
/// does not see); `Err`: a trailer is there and the program is damaged.
pub fn read<F: Read + Seek>(f: &mut F) -> Result<Option<Program>, String> {
    let end = f.seek(SeekFrom::End(0)).map_err(|e| e.to_string())?;
    if end < TRAILER_LEN {
        return Ok(None);
    }
    let want = (TRAILER_LEN + PAD_MAX).min(end);
    let tail = read_at(f, end - want, want)?;
    let zeros = tail.iter().rev().take_while(|b| **b == 0).count() as u64;
    let n = tail.len() as u64;
    // the trailer at the very end first; then past zero padding, which may be as long as it is
    for pad in 0..=zeros.min(PAD_MAX) {
        let Some(at) = n.checked_sub(pad + TRAILER_LEN) else {
            break;
        };
        let t = &tail[at as usize..(at + TRAILER_LEN) as usize];
        if t[..8] == MAGIC {
            let trailer_at = end - pad - TRAILER_LEN;
            return at_trailer(f, trailer_at, t).map(Some);
        }
    }
    Ok(None)
}

fn read_at<F: Read + Seek>(f: &mut F, at: u64, len: u64) -> Result<Vec<u8>, String> {
    f.seek(SeekFrom::Start(at)).map_err(|e| e.to_string())?;
    let mut b = Vec::new();
    f.by_ref()
        .take(len)
        .read_to_end(&mut b)
        .map_err(|e| e.to_string())?;
    if b.len() as u64 != len {
        return Err("the program ends before its trailer says it does".to_string());
    }
    Ok(b)
}

fn at_trailer<F: Read + Seek>(f: &mut F, at: u64, t: &[u8]) -> Result<Program, String> {
    let mut len_bytes = [0u8; 8];
    len_bytes.copy_from_slice(&t[8..16]);
    let len = u64::from_le_bytes(len_bytes);
    if len > MAX_PAYLOAD || len > at {
        return Err(format!(
            "the trailer says the program is {len} bytes, which this file cannot hold"
        ));
    }
    let payload = read_at(f, at - len, len)?;
    if Sha256::digest(&payload)[..] != t[16..] {
        return Err("the program's bytes do not match the sha256 in its trailer".to_string());
    }
    parse_payload(&payload)
}

/// The record in a payload, checked the way cig checks it before it runs one.
pub fn parse_payload(payload: &[u8]) -> Result<Program, String> {
    let v: serde_json::Value = serde_json::from_slice(payload)
        .map_err(|e| format!("the program's record does not read: {e}"))?;
    let text = |k: &str| -> Result<&str, String> {
        v.get(k)
            .and_then(|x| x.as_str())
            .ok_or_else(|| format!("the program's record has no `{k}`"))
    };
    let format = v.get("format").and_then(|x| x.as_u64());
    if format != Some(FORMAT) {
        return Err(match format {
            Some(n) => format!("the program is in format {n}; this reader knows format {FORMAT}"),
            None => "the program's record has no `format`".to_string(),
        });
    }
    let (name, file, source, sha256, cig, compiler) = (
        text("name")?,
        text("file")?,
        text("source")?,
        text("sha256")?,
        text("cig")?,
        text("compiler")?,
    );
    if hex::encode(Sha256::digest(source.as_bytes())) != sha256 {
        return Err("the script does not match the sha256 the program records".to_string());
    }
    if !good_name(name) {
        return Err(
            "the program's name is not a plain word (letters, digits, `.`, `_`, `-`)".to_string(),
        );
    }
    for (what, value, max) in [
        ("file", file, 4096),
        ("compiler", compiler, 128),
        ("cig", cig, 32),
    ] {
        if value.is_empty() || value.chars().count() > max || !printable(value) {
            return Err(format!("the program's `{what}` is not printable text"));
        }
    }
    Ok(Program {
        name: name.to_string(),
        file: file.to_string(),
        sha256: sha256.to_string(),
        cig: cig.to_string(),
        compiler: compiler.to_string(),
        source_bytes: source.len(),
    })
}

/// A program's name: a letter or digit, then letters, digits, `.`, `_` or `-`, 64 at most.
pub fn good_name(name: &str) -> bool {
    let b = name.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && b[0].is_ascii_alphanumeric()
        && b.iter()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
}

/// Text that shows as what it says: no control characters and none of Unicode's invisible ones
/// (format characters, the bidi controls among them, the line and paragraph separators).
pub fn printable(s: &str) -> bool {
    !s.chars().any(|c| c.is_control() || invisible(c))
}

fn invisible(c: char) -> bool {
    matches!(c as u32,
        0x00AD | 0x0600..=0x0605 | 0x061C | 0x06DD | 0x070F | 0x0890..=0x0891 | 0x08E2
        | 0x180E | 0x200B..=0x200F | 0x2028..=0x202E | 0x2060..=0x2064 | 0x2066..=0x206F
        | 0xFEFF | 0xFFF9..=0xFFFB | 0x110BD | 0x110CD | 0x13430..=0x1343F
        | 0x1BCA0..=0x1BCA3 | 0x1D173..=0x1D17A | 0xE0001 | 0xE0020..=0xE007F)
}

/// A program as its file's bytes: `runtime`, the record, the trailer. For tests and fixtures; the
/// record's fields are written in cig's order.
pub fn build(
    runtime: &[u8],
    name: &str,
    file: &str,
    source: &str,
    cig: &str,
    compiler: &str,
) -> Vec<u8> {
    let payload = serde_json::json!({
        "format": FORMAT,
        "name": name,
        "file": file,
        "source": source,
        "sha256": hex::encode(Sha256::digest(source.as_bytes())),
        "cig": cig,
        "compiler": compiler,
    });
    let payload = serde_json::to_vec(&payload).unwrap_or_default();
    let mut out = runtime.to_vec();
    out.extend_from_slice(&payload);
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    out.extend_from_slice(&Sha256::digest(&payload));
    out
}

/// The advisories of the manifest that cover this cig: an `affects` entry for subject `cig` whose range
/// matches it. Newest listed last, in the manifest's order.
pub fn advisories_for_cig<'a>(m: &'a Manifest, cig: &Version) -> Vec<&'a Advisory> {
    m.advisories
        .iter()
        .filter(|a| {
            a.affects
                .iter()
                .any(|x| x.subject == "cig" && x.range.matches_core(cig))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn prog() -> Vec<u8> {
        build(
            b"RUNTIME-BYTES",
            "tidy",
            "tools/tidy.cig",
            "say \"hi\"\n",
            "1.20.0",
            "cigcompile 0.2.0",
        )
    }

    #[test]
    fn a_whole_program_is_read() {
        let p = read(&mut Cursor::new(prog())).unwrap().unwrap();
        assert_eq!(p.name, "tidy");
        assert_eq!(p.cig, "1.20.0");
        assert_eq!(p.compiler, "cigcompile 0.2.0");
        assert_eq!(p.file, "tools/tidy.cig");
        assert_eq!(p.source_bytes, 9);
        assert_eq!(p.cig_version(), Version::parse("1.20.0").ok());
    }

    #[test]
    fn zero_padding_after_the_trailer_is_allowed_and_other_bytes_are_not_a_program() {
        let mut b = prog();
        b.extend_from_slice(&[0u8; 100]);
        assert!(read(&mut Cursor::new(b)).unwrap().is_some());
        let mut b = prog();
        b.extend_from_slice(b"tail");
        assert_eq!(read(&mut Cursor::new(b)).unwrap(), None);
        let mut b = prog();
        b.extend_from_slice(&vec![0u8; PAD_MAX as usize + 10]);
        assert_eq!(read(&mut Cursor::new(b)).unwrap(), None);
    }

    #[test]
    fn a_file_without_a_trailer_is_not_a_program() {
        assert_eq!(read(&mut Cursor::new(Vec::new())).unwrap(), None);
        assert_eq!(read(&mut Cursor::new(vec![1u8; 47])).unwrap(), None);
        assert_eq!(read(&mut Cursor::new(vec![7u8; 5000])).unwrap(), None);
    }

    #[test]
    fn damage_is_said_and_never_panics() {
        // a flipped payload byte
        let mut b = prog();
        let at = b.len() - 60;
        b[at] ^= 1;
        let e = read(&mut Cursor::new(b)).unwrap_err();
        assert!(e.contains("sha256"), "{e}");
        // a trailer that counts more bytes than the file has
        let mut b = prog();
        let n = b.len();
        b[n - 40..n - 32].copy_from_slice(&u64::MAX.to_le_bytes());
        let e = read(&mut Cursor::new(b)).unwrap_err();
        assert!(e.contains("cannot hold"), "{e}");
        // one that counts more than the limit
        let mut b = prog();
        let n = b.len();
        b[n - 40..n - 32].copy_from_slice(&(MAX_PAYLOAD + 1).to_le_bytes());
        assert!(read(&mut Cursor::new(b)).is_err());
        // a trailer alone
        let mut b = MAGIC.to_vec();
        b.extend_from_slice(&5u64.to_le_bytes());
        b.extend_from_slice(&[0u8; 32]);
        assert!(read(&mut Cursor::new(b)).is_err());
    }

    #[test]
    fn a_record_that_lies_is_refused() {
        let ok = |p: &[u8]| parse_payload(p);
        let rec = |edit: &dyn Fn(&mut serde_json::Value)| {
            let mut v: serde_json::Value = serde_json::from_slice(
                &serde_json::to_vec(&serde_json::json!({
                    "format": 1, "name": "tidy", "file": "a.cig", "source": "x",
                    "sha256": hex::encode(Sha256::digest(b"x")),
                    "cig": "1.20.0", "compiler": "cigcompile 0.2.0"
                }))
                .unwrap(),
            )
            .unwrap();
            edit(&mut v);
            serde_json::to_vec(&v).unwrap()
        };
        assert!(ok(&rec(&|_| {})).is_ok());
        assert!(ok(&rec(&|v| v["format"] = 2.into()))
            .unwrap_err()
            .contains("format 2"));
        assert!(ok(&rec(&|v| v["name"] = "../x".into()))
            .unwrap_err()
            .contains("name"));
        assert!(ok(&rec(&|v| v["cig"] = "1.2\u{202e}0".into()))
            .unwrap_err()
            .contains("cig"));
        assert!(ok(&rec(&|v| v["source"] = "y".into()))
            .unwrap_err()
            .contains("script"));
        assert!(ok(&rec(&|v| v["compiler"] = 3.into()))
            .unwrap_err()
            .contains("compiler"));
        assert!(ok(b"not json").is_err());
        assert!(ok(b"[1,2]").is_err());
        assert!(ok(b"").is_err());
    }

    #[test]
    fn names_follow_cigs_rule() {
        assert!(good_name("tidy-2.x_y"));
        assert!(!good_name(""));
        assert!(!good_name("-tidy"));
        assert!(!good_name(&"a".repeat(65)));
        assert!(!good_name("a b"));
    }

    #[test]
    fn every_prefix_and_every_flipped_byte_of_a_program_is_handled() {
        let whole = prog();
        for cut in 0..whole.len() {
            let _ = read(&mut Cursor::new(whole[..cut].to_vec()));
        }
        for i in (0..whole.len()).step_by(3) {
            let mut b = whole.clone();
            b[i] ^= 0xff;
            let _ = read(&mut Cursor::new(b));
        }
    }
}
