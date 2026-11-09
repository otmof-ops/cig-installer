// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0

//! A small RFC 3339 reader: enough to order `generated` against `expires` and to
//! let callers compare a manifest's expiry with their clock. No I/O, no clock.

fn digits(s: &str, n: usize) -> Option<u32> {
    if s.len() != n || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// Days since 1970-01-01 for a proleptic Gregorian date.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn month_days(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        _ => 28,
    }
}

/// Seconds since the Unix epoch for `2026-11-12T10:00:00Z` (also `t`/space as the
/// separator, fractional seconds, and `+HH:MM` offsets). `None` when it is not a
/// full date-time with an offset.
pub fn rfc3339_to_epoch(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 20 || !s.is_ascii() {
        return None;
    }
    let y = i64::from(digits(&s[0..4], 4)?);
    if &s[4..5] != "-" || &s[7..8] != "-" || !matches!(&s[10..11], "T" | "t" | " ") {
        return None;
    }
    let mo = digits(&s[5..7], 2)?;
    let d = digits(&s[8..10], 2)?;
    if &s[13..14] != ":" || &s[16..17] != ":" {
        return None;
    }
    let h = digits(&s[11..13], 2)?;
    let mi = digits(&s[14..16], 2)?;
    let se = digits(&s[17..19], 2)?;
    if !(1..=12).contains(&mo) || d == 0 || d > month_days(y, mo) || h > 23 || mi > 59 || se > 60 {
        return None;
    }
    let mut rest = &s[19..];
    if let Some(frac) = rest.strip_prefix('.') {
        let n = frac.bytes().take_while(u8::is_ascii_digit).count();
        if n == 0 {
            return None;
        }
        rest = &frac[n..];
    }
    let offset = match rest {
        "Z" | "z" => 0,
        _ => {
            let sign = match rest.as_bytes().first()? {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            if rest.len() != 6 || &rest[3..4] != ":" {
                return None;
            }
            let oh = i64::from(digits(&rest[1..3], 2)?);
            let om = i64::from(digits(&rest[4..6], 2)?);
            if oh > 23 || om > 59 {
                return None;
            }
            sign * (oh * 3600 + om * 60)
        }
    };
    let days = days_from_civil(y, mo, d);
    Some(days * 86_400 + i64::from(h) * 3600 + i64::from(mi) * 60 + i64::from(se) - offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_instants() {
        assert_eq!(rfc3339_to_epoch("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(rfc3339_to_epoch("2000-03-01T00:00:00Z"), Some(951_868_800));
        assert_eq!(
            rfc3339_to_epoch("2026-11-12T12:00:00+02:00"),
            rfc3339_to_epoch("2026-11-12T10:00:00Z")
        );
        assert_eq!(rfc3339_to_epoch("2026-11-12"), None);
        assert_eq!(rfc3339_to_epoch("2026-02-30T00:00:00Z"), None);
        assert_eq!(rfc3339_to_epoch("2026-11-12T10:00:00"), None);
    }
}
