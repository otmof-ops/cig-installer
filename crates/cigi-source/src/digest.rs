// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! sha256 helpers.

use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;

/// 128 random bits as hex (the std hasher's per-process random keys, mixed with pid, time and a
/// counter): names for files that must not be guessable. Not a secret, not a key.
pub fn random_suffix() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut out = String::new();
    for _ in 0..2 {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(nanos);
        h.write_u32(std::process::id());
        h.write_u64(N.fetch_add(1, Ordering::Relaxed));
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Hash a file; returns `(hex, length)`.
pub fn sha256_file(path: &Path) -> std::io::Result<(String, u64)> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut n_total = 0u64;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        n_total += n as u64;
    }
    Ok((hex::encode(h.finalize()), n_total))
}

pub fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
