// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0

//! Versions and pins. `Pin` is cig's own pin dialect, copied from
//! `cigscript::pin::Constraint` (cig v1.20.0); the `parity` feature runs a table
//! of inputs against the original.

use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

/// Why a version string did not parse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionError(pub String);

impl fmt::Display for VersionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for VersionError {}

/// Why a pin did not parse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinError(pub String);

impl fmt::Display for PinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for PinError {}

/// A semantic version: `major.minor.patch` with an optional prerelease.
/// Build metadata (`+x`) is refused so that `Eq` and `Ord` agree.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub pre: Option<String>,
}

/// A carton is numbered like a product (`0.N.P` until carton 1.0.0).
pub type CartonVersion = Version;

impl Version {
    pub fn new(major: u64, minor: u64, patch: u64) -> Version {
        Version {
            major,
            minor,
            patch,
            pre: None,
        }
    }

    /// `1.2.3`, `v1.2.3`, `1.2.3-beta.1`. `1.2` is not accepted here.
    pub fn parse(s: &str) -> Result<Version, VersionError> {
        let bad = |why: &str| VersionError(format!("`{s}` is not a version: {why}"));
        let body = s.strip_prefix('v').unwrap_or(s);
        let (core, pre) = match body.split_once('-') {
            Some((c, p)) => (c, Some(p)),
            None => (body, None),
        };
        let parts: Vec<&str> = core.split('.').collect();
        if parts.len() != 3 {
            return Err(bad("write major.minor.patch"));
        }
        let mut nums = [0u64; 3];
        for (slot, part) in nums.iter_mut().zip(&parts) {
            *slot = numeric(part).ok_or_else(|| bad("each part is a plain number"))?;
        }
        if let Some(p) = pre {
            if p.is_empty() {
                return Err(bad("empty prerelease"));
            }
            for id in p.split('.') {
                if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
                    return Err(bad("bad prerelease"));
                }
                if id.bytes().all(|b| b.is_ascii_digit()) && id.len() > 1 && id.starts_with('0') {
                    return Err(bad("numeric prerelease part with a leading zero"));
                }
            }
        }
        Ok(Version {
            major: nums[0],
            minor: nums[1],
            patch: nums[2],
            pre: pre.map(str::to_string),
        })
    }

    pub fn is_prerelease(&self) -> bool {
        self.pre.is_some()
    }

    /// The same version in `semver`'s type, for range matching.
    pub(crate) fn to_semver(&self) -> Option<semver::Version> {
        let pre = match &self.pre {
            Some(p) => semver::Prerelease::new(p).ok()?,
            None => semver::Prerelease::EMPTY,
        };
        Some(semver::Version {
            major: self.major,
            minor: self.minor,
            patch: self.patch,
            pre,
            build: semver::BuildMetadata::EMPTY,
        })
    }
}

/// A plain decimal number without a sign or a leading zero (`0` is fine).
fn numeric(s: &str) -> Option<u64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) || (s.len() > 1 && s.starts_with('0'))
    {
        return None;
    }
    s.parse().ok()
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(p) = &self.pre {
            write!(f, "-{p}")?;
        }
        Ok(())
    }
}

impl FromStr for Version {
    type Err = VersionError;
    fn from_str(s: &str) -> Result<Version, VersionError> {
        Version::parse(s)
    }
}

fn cmp_pre_id(a: &str, b: &str) -> Ordering {
    let an = a.bytes().all(|c| c.is_ascii_digit());
    let bn = b.bytes().all(|c| c.is_ascii_digit());
    match (an, bn) {
        // numeric identifiers: by value (no leading zeros, so length then text)
        (true, true) => a.len().cmp(&b.len()).then_with(|| a.cmp(b)),
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (false, false) => a.cmp(b),
    }
}

impl Ord for Version {
    /// Semver precedence: a prerelease sorts before its release.
    fn cmp(&self, other: &Version) -> Ordering {
        self.major
            .cmp(&other.major)
            .then(self.minor.cmp(&other.minor))
            .then(self.patch.cmp(&other.patch))
            .then_with(|| match (&self.pre, &other.pre) {
                (None, None) => Ordering::Equal,
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (Some(a), Some(b)) => {
                    let mut ai = a.split('.');
                    let mut bi = b.split('.');
                    loop {
                        match (ai.next(), bi.next()) {
                            (None, None) => return Ordering::Equal,
                            (None, Some(_)) => return Ordering::Less,
                            (Some(_), None) => return Ordering::Greater,
                            (Some(x), Some(y)) => {
                                let c = cmp_pre_id(x, y);
                                if c != Ordering::Equal {
                                    return c;
                                }
                            }
                        }
                    }
                }
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Version) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// cig's pin dialect (`cigscript::pin::Constraint`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pin {
    /// `1.1.2`: exactly that one.
    Exact(Version),
    /// `1.1`: any 1.1.x.
    Minor(u64, u64),
    /// `1`: any 1.x.
    Major(u64),
    /// `>=1.1.2`: that or newer, within the same major version.
    AtLeast(Version),
}

/// cigscript's `update::Version::parse`, byte for byte in behavior: `v` prefixes
/// are stripped, prerelease and build parts are dropped, a missing patch is 0.
fn lenient_version(s: &str) -> Option<Version> {
    let s = s.trim().trim_start_matches('v');
    let core = s.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let a = parts.next()?.parse().ok()?;
    let b = parts.next()?.parse().ok()?;
    let c = parts.next().unwrap_or("0").parse().ok()?;
    Some(Version::new(a, b, c))
}

impl Pin {
    /// `1.1`, `1.1.2`, `>=1.1.2`, `v1`, with or without `v`.
    pub fn parse(s: &str) -> Result<Pin, PinError> {
        let t = s.trim();
        if let Some(rest) = t.strip_prefix(">=") {
            return lenient_version(rest.trim())
                .map(Pin::AtLeast)
                .ok_or_else(|| PinError(format!("`{t}` is not a version constraint")));
        }
        let core = t.trim_start_matches('v');
        if core.is_empty()
            || !core.chars().all(|c| c.is_ascii_digit() || c == '.')
            || core.starts_with('.')
            || core.ends_with('.')
        {
            return Err(PinError(format!(
                "`{t}` is not a version constraint; write 1.1 (any 1.1.x), 1.1.2 (exactly) or >=1.1.2"
            )));
        }
        let parts: Vec<&str> = core.split('.').collect();
        let num = |p: &str| {
            p.parse::<u64>()
                .map_err(|_| PinError(format!("`{t}` is not a version constraint")))
        };
        match parts.len() {
            1 => Ok(Pin::Major(num(parts[0])?)),
            2 => Ok(Pin::Minor(num(parts[0])?, num(parts[1])?)),
            3 => Ok(Pin::Exact(Version::new(
                num(parts[0])?,
                num(parts[1])?,
                num(parts[2])?,
            ))),
            _ => Err(PinError(format!("`{t}` is not a version constraint"))),
        }
    }

    /// A `.cig-carton` line: `carton 0.1`, or just `0.1`.
    pub fn parse_carton_line(line: &str) -> Result<Pin, PinError> {
        let t = line.trim();
        let rest = match t.strip_prefix("carton") {
            Some(r) if r.starts_with(char::is_whitespace) => r.trim_start(),
            _ => t,
        };
        Pin::parse(rest)
    }

    /// Does this version satisfy the pin? No range crosses a major version, and a
    /// prerelease never satisfies a range.
    pub fn satisfied_by(&self, v: &Version) -> bool {
        match self {
            Pin::Exact(e) => v == e,
            Pin::Minor(a, b) => v.pre.is_none() && v.major == *a && v.minor == *b,
            Pin::Major(a) => v.pre.is_none() && v.major == *a,
            Pin::AtLeast(m) => v.pre.is_none() && v.major == m.major && v >= m,
        }
    }

    pub fn is_exact(&self) -> bool {
        matches!(self, Pin::Exact(_))
    }

    /// The least version that could satisfy the pin.
    pub fn floor(&self) -> Version {
        match self {
            Pin::Exact(e) | Pin::AtLeast(e) => e.clone(),
            Pin::Minor(a, b) => Version::new(*a, *b, 0),
            Pin::Major(a) => Version::new(*a, 0, 0),
        }
    }
}

impl fmt::Display for Pin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Pin::Exact(v) => write!(f, "{v}"),
            Pin::Minor(a, b) => write!(f, "{a}.{b}"),
            Pin::Major(a) => write!(f, "{a}"),
            Pin::AtLeast(v) => write!(f, ">={v}"),
        }
    }
}

impl FromStr for Pin {
    type Err = PinError;
    fn from_str(s: &str) -> Result<Pin, PinError> {
        Pin::parse(s)
    }
}
