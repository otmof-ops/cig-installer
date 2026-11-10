// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! Staged rollouts (#17): `rollout = N%` lets a share of machines take a new carton on their own
//! first, so a bad carton is caught on a few machines.
//!
//! A machine's *bucket* (0 to 99) is a hash of the operating system's machine id with a fixed
//! prefix, so it is stable, it matches no other hash of that id, and it cannot be turned back
//! into the id. The id and the bucket are used here and nowhere else: they are never printed,
//! written to a file, put in a report or sent in a request.
//!
//! The hash, the bucket and the decision are pure functions of the id, so they are tested
//! directly. Reading the id from the system is the only part that touches the machine.
//!
//! # Test seam
//!
//! `CIGI_TEST_MACHINE_ID` stands in for the system's id on this run. It exists so tests can give
//! the binary a synthetic id. All it can do is move this machine's own bucket: it is not a
//! setting, no file can carry it, and nothing reads it for any other purpose.

use sha2::{Digest, Sha256};

/// The environment variable that replaces the system's machine id (a test seam, see above).
pub const ID_SEAM: &str = "CIGI_TEST_MACHINE_ID";

/// Fixed prefix of the hash: separates this use of the id from every other.
const DOMAIN: &[u8] = b"cigi/rollout/v1\0";

/// Why a machine is outside the share, when that is not simply its bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// The machine has no usable id, so it has no stable bucket.
    NoMachineId,
    /// The `rollout` setting is not a percent from 1 to 100.
    InvalidSetting,
}

impl Why {
    /// The words `status --json` carries.
    pub fn as_str(self) -> &'static str {
        match self {
            Why::NoMachineId => "no-machine-id",
            Why::InvalidSetting => "invalid-setting",
        }
    }
}

/// Where this machine stands for the rollout in force.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Standing {
    /// The share, 1 to 100; `None` when the setting could not be read as one.
    pub percent: Option<u8>,
    pub in_share: bool,
    pub why: Option<Why>,
    /// The setting as written, for a message about a bad one.
    pub raw: String,
}

impl Standing {
    /// Is the rollout open to everyone (the default)?
    pub fn is_full(&self) -> bool {
        self.percent == Some(100)
    }

    /// The share in words for a message: `10%`, or the bad text in backticks.
    pub fn share_text(&self) -> String {
        match self.percent {
            Some(p) => format!("{p}%"),
            None => format!("`{}` (not a percent from 1 to 100)", self.raw),
        }
    }
}

/// `10`, `10%` or ` 10 % ` is 10; anything outside 1 to 100, or not a whole number, is `None`.
pub fn parse_percent(value: &str) -> Option<u8> {
    let v = value.trim();
    let v = v.strip_suffix('%').unwrap_or(v).trim();
    if v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    v.parse::<u8>().ok().filter(|n| (1..=100).contains(n))
}

/// An id as it is compared: trimmed and lowercase (macOS prints its UUID in capitals, Windows
/// in lowercase). Empty and the placeholder systemd writes before a first boot are no id.
pub fn clean_id(raw: &str) -> Option<String> {
    let id = raw.trim().to_ascii_lowercase();
    (!id.is_empty() && id != "uninitialized").then_some(id)
}

/// The hash that makes a bucket: SHA-256 of the domain prefix and the cleaned id.
fn digest(id: &str) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(DOMAIN);
    h.update(id.as_bytes());
    h.finalize().into()
}

/// The machine's bucket, 0 to 99. The same id always gives the same bucket. Never print it.
pub fn bucket(id: &str) -> u8 {
    let d = digest(&clean_id(id).unwrap_or_default());
    let n = u64::from_be_bytes([d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]]);
    (n % 100) as u8
}

/// Is this machine in a share of `percent`? 100 is everyone, id or not. Below 100 a machine
/// with no id is outside: with nothing stable to hash, waiting is the cautious side.
pub fn in_share(id: Option<&str>, percent: u8) -> bool {
    if percent >= 100 {
        return true;
    }
    id.and_then(clean_id).is_some_and(|i| bucket(&i) < percent)
}

/// The decision for a setting text and an id, with the reason when it is not the bucket.
pub fn standing_with(setting: &str, id: Option<&str>) -> Standing {
    let raw = setting.to_string();
    let Some(p) = parse_percent(setting) else {
        return Standing {
            percent: None,
            in_share: false,
            why: Some(Why::InvalidSetting),
            raw,
        };
    };
    let has_id = id.and_then(clean_id).is_some();
    let yes = in_share(id, p);
    Standing {
        percent: Some(p),
        in_share: yes,
        why: (!yes && !has_id).then_some(Why::NoMachineId),
        raw,
    }
}

/// The decision on this machine. The id is read only when the share is below 100.
pub fn standing(setting: &str) -> Standing {
    if parse_percent(setting) == Some(100) {
        return standing_with(setting, None);
    }
    standing_with(setting, machine_id().as_deref())
}

/// This machine's id from the operating system (or the test seam), cleaned; `None` when there
/// is none to be had.
pub fn machine_id() -> Option<String> {
    if let Ok(v) = std::env::var(ID_SEAM) {
        return clean_id(&v);
    }
    system_id()
}

#[cfg(windows)]
fn system_id() -> Option<String> {
    cigi_sys::env::read_machine_guid()
        .ok()
        .flatten()
        .and_then(|g| clean_id(&g))
}

#[cfg(target_os = "macos")]
fn system_id() -> Option<String> {
    let out = std::process::Command::new("/usr/sbin/ioreg")
        .args(["-rd1", "-c", "IOPlatformExpertDevice"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| parse_ioreg(&String::from_utf8_lossy(&out.stdout)))
        .flatten()
}

#[cfg(not(any(windows, target_os = "macos")))]
fn system_id() -> Option<String> {
    ["/etc/machine-id", "/var/lib/dbus/machine-id"]
        .iter()
        .find_map(|p| std::fs::read_to_string(p).ok().and_then(|t| clean_id(&t)))
}

/// The `IOPlatformUUID` out of `ioreg -rd1 -c IOPlatformExpertDevice` output.
pub fn parse_ioreg(text: &str) -> Option<String> {
    let line = text.lines().find(|l| l.contains("\"IOPlatformUUID\""))?;
    let (_, value) = line.split_once('=')?;
    clean_id(value.trim().trim_matches('"'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_text() {
        for (t, n) in [
            ("10%", 10),
            ("10", 10),
            (" 7 % ", 7),
            ("100%", 100),
            ("1", 1),
        ] {
            assert_eq!(parse_percent(t), Some(n), "{t}");
        }
        for t in [
            "0", "0%", "101", "-5", "10.5", "ten", "", "%", "10%%", "1e1", "+10", "256", "300%",
        ] {
            assert_eq!(parse_percent(t), None, "{t:?}");
        }
    }

    #[test]
    fn the_same_id_always_gives_the_same_bucket() {
        let a = bucket("0123456789abcdef0123456789abcdef");
        for _ in 0..50 {
            assert_eq!(bucket("0123456789abcdef0123456789abcdef"), a);
        }
        // case and surrounding space do not move a machine
        assert_eq!(bucket("  0123456789ABCDEF0123456789abcdef\n"), a);
        assert!(a < 100);
    }

    #[test]
    fn the_hash_is_pinned_so_a_change_to_it_is_deliberate() {
        // a changed prefix or reduction would move every machine in a running rollout
        let d = digest("abc");
        let again = {
            let mut h = Sha256::new();
            h.update(b"cigi/rollout/v1\0abc");
            h.finalize()
        };
        assert_eq!(d.as_slice(), again.as_slice());
        // and it is not the plain hash of the id, which other tools might compute
        assert_ne!(d.as_slice(), Sha256::digest(b"abc").as_slice());
    }

    #[test]
    fn buckets_spread_evenly() {
        let mut counts = [0u32; 10];
        let n = 20_000;
        for i in 0..n {
            let id = format!("{:032x}", (i as u128).wrapping_mul(0x9E37_79B9_7F4A_7C15));
            counts[(bucket(&id) / 10) as usize] += 1;
        }
        // each decile holds about 2000; allow a wide band, this is about gross skew only
        for (d, c) in counts.iter().enumerate() {
            assert!(
                (1700..=2300).contains(c),
                "decile {d} holds {c}: {counts:?}"
            );
        }
    }

    #[test]
    fn a_share_is_the_buckets_below_it() {
        let ids: Vec<String> = (0..2000).map(|i| format!("machine-{i}")).collect();
        for p in [1u8, 10, 25, 50, 99] {
            let n = ids.iter().filter(|i| in_share(Some(i), p)).count();
            let want = 20 * p as usize;
            assert!(n.abs_diff(want) <= 120, "{p}% took {n} of 2000");
        }
        // everyone at 100, with or without an id
        assert!(ids.iter().all(|i| in_share(Some(i), 100)));
        assert!(in_share(None, 100));
        // and a wider share only adds machines
        for i in &ids {
            if in_share(Some(i), 10) {
                assert!(in_share(Some(i), 11));
            }
        }
    }

    #[test]
    fn no_id_waits_below_100() {
        for none in [None, Some(""), Some("  \n"), Some("uninitialized\n")] {
            assert!(!in_share(none, 99), "{none:?}");
            assert!(!in_share(none, 1), "{none:?}");
            assert!(in_share(none, 100), "{none:?}");
        }
        let s = standing_with("50%", None);
        assert!(!s.in_share);
        assert_eq!(s.why, Some(Why::NoMachineId));
        let s = standing_with("100", None);
        assert!(s.in_share && s.why.is_none() && s.is_full());
    }

    #[test]
    fn a_bad_setting_waits_and_says_so() {
        let s = standing_with("banana", Some("some-id"));
        assert!(!s.in_share);
        assert_eq!((s.percent, s.why), (None, Some(Why::InvalidSetting)));
        assert!(s.share_text().contains("banana"));
        assert!(!standing_with("0", Some("x")).in_share);
    }

    #[test]
    fn standing_names_a_machine_in_or_out_by_its_bucket() {
        let id = "0123456789abcdef0123456789abcdef";
        let b = bucket(id);
        let inside = standing_with(&format!("{}", b as u32 + 1), Some(id));
        assert!(inside.in_share, "bucket {b} is below {}", b as u32 + 1);
        if b > 0 {
            let outside = standing_with(&format!("{b}"), Some(id));
            assert!(!outside.in_share && outside.why.is_none());
        }
    }

    #[test]
    fn ids_are_cleaned() {
        assert_eq!(clean_id(" AbC \n").as_deref(), Some("abc"));
        assert_eq!(clean_id("uninitialized"), None);
        assert_eq!(clean_id(""), None);
    }

    #[test]
    fn the_macos_id_is_read_from_ioreg_text() {
        let t = "+-o Root  <class IORegistryEntry>\n  +-o MacBookPro  <class IOPlatformExpertDevice>\n    {\n      \"IOPlatformSerialNumber\" = \"C02XXXX\"\n      \"IOPlatformUUID\" = \"ABCDEF01-2345-6789-ABCD-EF0123456789\"\n    }\n";
        assert_eq!(
            parse_ioreg(t).as_deref(),
            Some("abcdef01-2345-6789-abcd-ef0123456789")
        );
        assert_eq!(parse_ioreg("nothing here"), None);
    }
}
