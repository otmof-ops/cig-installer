// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0
//! Time as this crate needs it: parse an RFC 3339 stamp, compare with now.
//!
//! Section 1 says time is written by `cigi-home` only. This crate only needs to *read* the
//! manifest's `expires`, and to stamp `fetched.json`; the stamp goes through
//! `now_rfc3339` so the integrator can point it at `cigi-home` in one place.

use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

/// Seconds since the epoch.
pub type Epoch = i64;

pub fn now() -> Epoch {
    OffsetDateTime::now_utc().unix_timestamp()
}

pub fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .replace_nanosecond(0)
        .unwrap_or_else(|_| OffsetDateTime::now_utc())
        .format(&Rfc3339)
        .unwrap_or_default()
}

/// Parses `2026-12-12T10:00:00Z` (also a bare `2026-12-12`, which TOML dates and hand edits give).
pub fn parse(s: &str) -> Option<Epoch> {
    if let Ok(t) = OffsetDateTime::parse(s, &Rfc3339) {
        return Some(t.unix_timestamp());
    }
    let fmt = time::macros::format_description!("[year]-[month]-[day]");
    time::Date::parse(s, &fmt)
        .ok()
        .map(|d| d.midnight().assume_utc().unix_timestamp())
}

pub fn format(epoch: Epoch) -> Option<String> {
    OffsetDateTime::from_unix_timestamp(epoch)
        .ok()?
        .format(&Rfc3339)
        .ok()
}
