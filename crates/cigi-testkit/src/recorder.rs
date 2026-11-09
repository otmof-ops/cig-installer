// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0
//! The request recorder and the no-telemetry rule (#19, section 14.1).

use std::collections::BTreeSet;

/// One request as the fixture server saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorded {
    pub method: String,
    /// The path only, no query (`/manifest-stable.toml`).
    pub path: String,
    pub query: Option<String>,
    /// Header names as sent, values as sent.
    pub headers: Vec<(String, String)>,
}

impl Recorded {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

const ALLOWED_HEADERS: &[&str] = &[
    "host",
    "user-agent",
    "accept",
    "accept-encoding",
    "connection",
    "range",
];

/// The #19 rules over a recorded log. `known_files` are the file names in `releases/`; a request
/// path must be one of them, or a manifest or sidecar name (those may legitimately 404, such as an
/// attestation that was never published). Returns every violation found.
pub fn check_no_telemetry(
    requests: &[Recorded],
    known_files: &BTreeSet<String>,
) -> Result<(), Vec<String>> {
    let mut bad = Vec::new();
    for r in requests {
        let at = format!("{} {}", r.method, r.path);
        if r.method != "GET" {
            bad.push(format!("{at}: method is not GET"));
        }
        if let Some(q) = &r.query {
            bad.push(format!("{at}: has a query string ({q})"));
        }
        for (k, _) in &r.headers {
            let lk = k.to_ascii_lowercase();
            if lk == "cookie" || lk == "authorization" {
                bad.push(format!("{at}: sends {k}"));
            } else if !ALLOWED_HEADERS.contains(&lk.as_str()) {
                bad.push(format!("{at}: sends header {k} outside the allowed set"));
            }
        }
        match r.header("user-agent") {
            Some("ciginstaller") => {}
            Some(other) => bad.push(format!(
                "{at}: User-Agent is {other:?}, not exactly \"ciginstaller\""
            )),
            None => bad.push(format!("{at}: no User-Agent")),
        }
        let name = r.path.strip_prefix('/').unwrap_or(&r.path);
        let plain = !name.is_empty() && !name.contains('/');
        let allowed = plain
            && (known_files.contains(name)
                || name.starts_with("manifest-")
                || name.ends_with(".sha256"));
        if !allowed {
            bad.push(format!("{at}: not a manifest or asset path"));
        }
    }
    if bad.is_empty() {
        Ok(())
    } else {
        Err(bad)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(path: &str, headers: &[(&str, &str)]) -> Recorded {
        Recorded {
            method: "GET".into(),
            path: path.into(),
            query: None,
            headers: headers
                .iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect(),
        }
    }
    fn files() -> BTreeSet<String> {
        ["cig-v1.0.0-linux-x86_64".to_string()]
            .into_iter()
            .collect()
    }
    const GOOD: &[(&str, &str)] = &[
        ("Host", "127.0.0.1"),
        ("User-Agent", "ciginstaller"),
        ("Accept", "*/*"),
    ];

    #[test]
    fn clean_log_passes() {
        let log = [
            req("/manifest-stable.toml", GOOD),
            req("/cig-v1.0.0-linux-x86_64", GOOD),
            req("/manifest-stable.toml.attestation.jsonl", GOOD),
        ];
        assert_eq!(check_no_telemetry(&log, &files()), Ok(()));
    }

    #[test]
    fn each_rule_trips() {
        let mut q = req("/manifest-stable.toml", GOOD);
        q.query = Some("id=1".into());
        let mut post = req("/manifest-stable.toml", GOOD);
        post.method = "POST".into();
        let cases = [
            q,
            post,
            req(
                "/manifest-stable.toml",
                &[("User-Agent", "ciginstaller"), ("Cookie", "a=b")],
            ),
            req(
                "/manifest-stable.toml",
                &[
                    ("User-Agent", "ciginstaller"),
                    ("Authorization", "Bearer x"),
                ],
            ),
            req(
                "/manifest-stable.toml",
                &[("User-Agent", "ciginstaller"), ("X-Machine-Id", "abc")],
            ),
            req(
                "/manifest-stable.toml",
                &[("User-Agent", "ciginstaller/0.1")],
            ),
            req("/manifest-stable.toml", &[]),
            req("/api/telemetry", GOOD),
            req("/a/b", GOOD),
        ];
        for c in cases {
            assert!(
                check_no_telemetry(std::slice::from_ref(&c), &files()).is_err(),
                "should trip: {c:?}"
            );
        }
    }
}
