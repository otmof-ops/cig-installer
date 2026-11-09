// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0
//! The `.sha256` sidecar: `<hex>  <name>` as `sha256sum` writes it. Like cig's
//! `update::verify_sha256`, the first whitespace-separated token is the digest, lowercased.
//! Unlike cig's, the token must really be 64 hex digits: a sidecar that is an HTML error page
//! or a truncated line must not be compared, it must be refused.

use crate::error::SidecarError;

/// The digest of a sidecar, lowercase.
pub fn parse(text: &str) -> Result<String, SidecarError> {
    parse_named(text).map(|(d, _)| d)
}

/// The digest, and the file name if the sidecar carries one (`*name` binary-mode marker removed).
pub fn parse_named(text: &str) -> Result<(String, Option<String>), SidecarError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .ok_or(SidecarError::Empty)?;
    let mut it = line.split_whitespace();
    let token = it.next().ok_or(SidecarError::Empty)?;
    let digest = token.to_ascii_lowercase();
    if !crate::digest::is_sha256_hex(&digest) {
        let shown: String = token.chars().take(70).collect();
        return Err(SidecarError::NotADigest(shown));
    }
    let name = it
        .next()
        .map(|n| n.strip_prefix('*').unwrap_or(n).to_string());
    Ok((digest, name))
}

#[cfg(test)]
mod tests {
    use super::*;
    const D: &str = "4649fda09236ada8a2ade2460c33db9cfa4b2f4dba40186099118a6a3b2dc8e3";

    #[test]
    fn two_space_form() {
        assert_eq!(
            parse(&format!("{D}  cig-v1.20.0-linux-x86_64\n")).unwrap(),
            D
        );
    }
    #[test]
    fn bare_digest_and_crlf() {
        assert_eq!(parse(&format!("{D}\r\n")).unwrap(), D);
        assert_eq!(parse(D).unwrap(), D);
    }
    #[test]
    fn uppercase_is_lowered() {
        assert_eq!(parse(&D.to_ascii_uppercase()).unwrap(), D);
    }
    #[test]
    fn binary_marker_and_name() {
        let (d, n) = parse_named(&format!("{D} *file.tar.gz\n")).unwrap();
        assert_eq!((d.as_str(), n.as_deref()), (D, Some("file.tar.gz")));
    }
    #[test]
    fn blank_lines_first_are_skipped() {
        assert_eq!(parse(&format!("\n\n{D}  x\n")).unwrap(), D);
    }
    #[test]
    fn refuses_garbage() {
        assert_eq!(parse(""), Err(SidecarError::Empty));
        assert_eq!(parse("  \n"), Err(SidecarError::Empty));
        assert!(matches!(
            parse("<html>not found</html>"),
            Err(SidecarError::NotADigest(_))
        ));
        assert!(matches!(parse(&D[..63]), Err(SidecarError::NotADigest(_))));
        assert!(matches!(
            parse(&format!("{D}0")),
            Err(SidecarError::NotADigest(_))
        ));
        let mut z = D.to_string();
        z.replace_range(0..1, "g");
        assert!(matches!(parse(&z), Err(SidecarError::NotADigest(_))));
    }
}
