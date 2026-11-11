// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! `installer/config` (section 5.2): `key = value` lines, cig's dialect.

use std::collections::BTreeMap;

use crate::error::StateError;
use crate::home::Home;
use crate::journal::Txn;

/// One known key: allowed values (`None` = integer) and the default.
#[derive(Debug, Clone, Copy)]
pub struct KeySpec {
    pub key: &'static str,
    pub values: Option<&'static [&'static str]>,
    pub default: &'static str,
}

/// The keys of 5.2, with their defaults. `mirror` and `preset` default to unset.
pub const KEYS: &[KeySpec] = &[
    KeySpec {
        key: "channel",
        values: Some(&["stable", "beta", "nightly"]),
        default: "stable",
    },
    KeySpec {
        key: "auto_update",
        values: Some(&["off", "notify", "security", "all"]),
        default: "notify",
    },
    KeySpec {
        key: "update_check",
        values: Some(&["on", "off"]),
        default: "on",
    },
    KeySpec {
        key: "pin_fetch",
        values: Some(&["auto", "ask", "never"]),
        default: "ask",
    },
    KeySpec {
        key: "trust",
        values: Some(&["pinned", "attested", "checksummed"]),
        default: "checksummed",
    },
    KeySpec {
        key: "allow_single_source",
        values: Some(&["true", "false"]),
        default: "false",
    },
    KeySpec {
        key: "keep_days",
        values: None,
        default: "90",
    },
    KeySpec {
        key: "mirror",
        values: None,
        default: "",
    },
    KeySpec {
        key: "reports",
        values: Some(&["ask", "always", "never"]),
        default: "ask",
    },
    KeySpec {
        key: "preset",
        values: Some(&["", "chill", "balanced", "strict", "paranoid"]),
        default: "",
    },
    KeySpec {
        key: "bench_after_update",
        values: Some(&["on", "off"]),
        default: "off",
    },
    KeySpec {
        key: "rollout",
        values: None,
        default: "100",
    },
    KeySpec {
        key: "theme",
        values: Some(&["auto", "dark", "light"]),
        default: "auto",
    },
    KeySpec {
        key: "voice",
        values: Some(&["voice", "plain"]),
        default: "voice",
    },
];

fn spec(key: &str) -> Option<&'static KeySpec> {
    KEYS.iter().find(|k| k.key == key)
}

/// The installer's own settings. The file text is kept, so comments and
/// unknown keys survive a `set`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    text: String,
    values: BTreeMap<String, String>,
    env: BTreeMap<String, String>,
}

/// The host of a `mirror` value that starts with `http://` (any case), else `None`.
pub fn plain_http_host(value: &str) -> Option<String> {
    let v = value.trim();
    let rest = v.get(..7).filter(|p| p.eq_ignore_ascii_case("http://"))?;
    let rest = &v[rest.len()..];
    let auth = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = auth.rsplit('@').next().unwrap_or("");
    let host = match host.strip_prefix('[') {
        Some(h6) => format!("[{}]", h6.split(']').next().unwrap_or("")),
        None => host.split(':').next().unwrap_or("").to_string(),
    };
    Some(host.to_ascii_lowercase())
}

/// Is a host from `plain_http_host` a loopback address? A name never is, `localhost` included.
pub fn loopback_host(host: &str) -> bool {
    if host == "[::1]" {
        return true;
    }
    host.parse::<std::net::Ipv4Addr>()
        .is_ok_and(|a| a.is_loopback())
}

impl Config {
    /// Read `installer/config` (absent is fine) and the `CIGI_<KEY>` environment.
    pub fn load(home: &Home) -> Result<Config, StateError> {
        let p = home.config_path();
        let text = match std::fs::read_to_string(&p) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(source) => return Err(StateError::Io { path: p, source }),
        };
        Ok(Config::from_text(&text, &|k| std::env::var(k).ok()))
    }

    /// Read the file again, keeping this run's overrides (`set_for_run`) and the environment.
    /// For a writer that now holds the lock: the text it edits must be the file as it is now,
    /// not as it was when the command started.
    pub fn reread(&mut self, home: &Home) -> Result<(), StateError> {
        let fresh = Config::load(home)?;
        self.text = fresh.text;
        self.values = fresh.values;
        Ok(())
    }

    /// Parse config text with an explicit environment (tests).
    pub fn from_text(text: &str, env: &dyn Fn(&str) -> Option<String>) -> Config {
        let mut values = BTreeMap::new();
        for line in text.lines() {
            if let Some((k, v)) = parse_line(line) {
                values.insert(k, v);
            }
        }
        let mut envs = BTreeMap::new();
        for k in KEYS {
            let name = format!("CIGI_{}", k.key.to_ascii_uppercase());
            if let Some(v) = env(&name) {
                envs.insert(k.key.to_string(), v);
            }
        }
        Config {
            text: text.to_string(),
            values,
            env: envs,
        }
    }

    /// The effective value: environment, then the file, then the default.
    /// Unknown keys give an empty string.
    pub fn get(&self, key: &str) -> String {
        if let Some(v) = self.env.get(key) {
            return v.clone();
        }
        if let Some(v) = self.values.get(key) {
            return v.clone();
        }
        spec(key).map(|s| s.default.to_string()).unwrap_or_default()
    }

    /// A command-line flag for this run only (`install --channel beta`): validated like a
    /// setting, above the environment and the file, and never written.
    pub fn set_for_run(&mut self, key: &str, value: &str) -> Result<(), StateError> {
        Config::validate(key, value)?;
        self.env.insert(key.to_string(), value.to_string());
        Ok(())
    }

    /// The value in the file itself, ignoring the environment.
    pub fn get_file(&self, key: &str) -> Option<String> {
        self.values.get(key).cloned()
    }

    /// Is this key set by its `CIGI_<KEY>` environment variable (or a flag for this run)?
    pub fn env_has(&self, key: &str) -> bool {
        self.env.contains_key(key)
    }

    /// Where the effective value of `key` comes from: `environment`, `file` or `default`.
    pub fn source(&self, key: &str) -> &'static str {
        if self.env.contains_key(key) {
            "environment"
        } else if self.values.contains_key(key) {
            "file"
        } else {
            "default"
        }
    }

    /// Remove a key's lines from the file text, through the journal. Returns whether the file
    /// had one; when it had none nothing is written. Comments and other keys are untouched.
    pub fn unset_in(&mut self, txn: &mut Txn, key: &str) -> Result<bool, StateError> {
        if spec(key).is_none() {
            return Err(StateError::Usage(format!("unknown setting `{key}`")));
        }
        let mut out = String::new();
        let mut found = false;
        for line in self.text.lines() {
            match parse_line(line) {
                Some((k, _)) if k == key => found = true,
                _ => {
                    out.push_str(line);
                    out.push('\n');
                }
            }
        }
        if !found {
            return Ok(false);
        }
        let p = txn.home().config_path();
        txn.write(&p, out.as_bytes(), Some(0o644))?;
        self.text = out;
        self.values.remove(key);
        Ok(true)
    }

    /// The whole-number range a numeric key accepts, inclusive. Every numeric key in [`KEYS`]
    /// is listed here; a key with no entry is text or a fixed list of words.
    pub fn numeric_range(key: &str) -> Option<(u32, u32)> {
        match key {
            // 0 would make every carton "unused" and `gc` remove them all; ten years is the ceiling
            "keep_days" => Some((1, 3650)),
            // a share of machines, in percent; `10` and `10%` both read as ten
            "rollout" => Some((1, 100)),
            _ => None,
        }
    }

    /// Check a key and value against 5.2 (`E1870` otherwise).
    pub fn validate(key: &str, value: &str) -> Result<(), StateError> {
        let s = spec(key).ok_or_else(|| StateError::Usage(format!("unknown setting `{key}`")))?;
        if value.contains('\n') || value.contains('\r') {
            return Err(StateError::Usage(format!(
                "`{key}` cannot hold a line break"
            )));
        }
        match s.values {
            Some(allowed) if !allowed.contains(&value) => Err(StateError::Usage(format!(
                "`{key}` is one of {}; got `{value}`",
                allowed
                    .iter()
                    .filter(|v| !v.is_empty())
                    .copied()
                    .collect::<Vec<_>>()
                    .join(", ")
            ))),
            None if key == "mirror" => match plain_http_host(value) {
                Some(host) if !loopback_host(&host) => Err(StateError::PlainHttp(format!(
                    "`mirror` is an https address (plain http only to a loopback address, and then only for builds a pin vouches for); got `{value}`"
                ))),
                _ => Ok(()),
            },
            None if key == "rollout" => {
                if crate::rollout::parse_percent(value).is_some() {
                    Ok(())
                } else {
                    Err(StateError::Usage(format!(
                        "`rollout` is a share of machines from 1 to 100 percent, like 10%; got `{value}`"
                    )))
                }
            }
            None => match Config::numeric_range(key) {
                Some((lo, hi)) if !value.parse::<u32>().is_ok_and(|n| (lo..=hi).contains(&n)) => {
                    Err(StateError::Usage(format!(
                        "`{key}` is a whole number from {lo} to {hi}; got `{value}`"
                    )))
                }
                _ => Ok(()),
            },
            _ => Ok(()),
        }
    }

    /// Set a key in the file text and write it through the journal. The line
    /// is replaced in place when present, else appended; other lines and
    /// comments are untouched.
    pub fn set_in(&mut self, txn: &mut Txn, key: &str, value: &str) -> Result<(), StateError> {
        Config::validate(key, value)?;
        let new_line = format!("{key} = {value}");
        let mut out = String::new();
        let mut done = false;
        for line in self.text.lines() {
            match parse_line(line) {
                Some((k, _)) if k == key => {
                    if !done {
                        out.push_str(&new_line);
                        out.push('\n');
                        done = true;
                    }
                }
                _ => {
                    out.push_str(line);
                    out.push('\n');
                }
            }
        }
        if !done {
            out.push_str(&new_line);
            out.push('\n');
        }
        let p = txn.home().config_path();
        txn.write(&p, out.as_bytes(), Some(0o644))?;
        self.text = out;
        self.values.insert(key.to_string(), value.to_string());
        Ok(())
    }
}

/// `key = value`, `#` comments, optional single or double quotes.
fn parse_line(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let (k, v) = line.split_once('=')?;
    let k = k.trim();
    if k.is_empty() {
        return None;
    }
    let mut v = v.trim();
    for q in ['"', '\''] {
        if v.len() >= 2 && v.starts_with(q) && v.ends_with(q) {
            v = &v[1..v.len() - 1];
            break;
        }
    }
    Some((k.to_string(), v.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn reread_takes_the_file_as_it_is_now_and_keeps_this_runs_flags() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let home = Home::at(tmp.path().join("h"));
        home.ensure_layout().expect("layout");
        std::fs::write(
            home.config_path(),
            "# mine
",
        )
        .expect("write");
        let mut c = Config::load(&home).expect("load");
        c.set_for_run("channel", "beta").expect("flag");
        // another writer edits the file after this one read it
        std::fs::write(home.config_path(), "# mine\nmirror = \"file:///m\"\n").expect("write");
        assert_eq!(c.get("mirror"), "");
        c.reread(&home).expect("reread");
        assert_eq!(c.get("mirror"), "file:///m");
        assert_eq!(c.get("channel"), "beta", "the run's own flag survives");
    }

    #[test]
    fn defaults_file_and_env_in_that_precedence() {
        let c = Config::from_text("# c\nchannel = beta\nmirror = \"file:///m\"\n", &no_env);
        assert_eq!(c.get("channel"), "beta");
        assert_eq!(c.get("mirror"), "file:///m");
        assert_eq!(c.get("auto_update"), "notify");
        assert_eq!(c.get("keep_days"), "90");
        assert_eq!(c.get("nope"), "");
        let c = Config::from_text("channel = beta\n", &|k| {
            (k == "CIGI_CHANNEL").then(|| "nightly".to_string())
        });
        assert_eq!(c.get("channel"), "nightly");
        assert_eq!(c.get_file("channel").as_deref(), Some("beta"));
    }

    #[test]
    fn source_says_where_the_value_comes_from() {
        let c = Config::from_text("channel = beta\n", &|k| {
            (k == "CIGI_THEME").then(|| "dark".to_string())
        });
        assert_eq!(c.source("channel"), "file");
        assert_eq!(c.source("theme"), "environment");
        assert_eq!(c.source("keep_days"), "default");
        assert!(c.env_has("theme") && !c.env_has("channel"));
    }

    #[test]
    fn every_numeric_key_has_a_range() {
        // keys with no list of words are numeric, except the text ones named here
        for k in KEYS.iter().filter(|k| k.values.is_none()) {
            if k.key == "mirror" {
                continue;
            }
            assert!(
                Config::numeric_range(k.key).is_some(),
                "{} has no list of values and no range",
                k.key
            );
        }
    }

    #[test]
    fn a_plain_http_mirror_is_refused_unless_loopback() {
        for bad in [
            "http://example.com/m",
            "HTTP://example.com",
            "http://192.168.1.5:8080/",
            "http://localhost/",
            "http://127.0.0.1@example.com/",
            " http://example.com ",
        ] {
            let e = Config::validate("mirror", bad).unwrap_err();
            assert_eq!(e.code(), "E1900", "{bad}");
        }
        for good in [
            "https://example.com/m",
            "file:///m",
            "/srv/mirror",
            "http://127.0.0.1:8080/",
            "http://[::1]:9/",
        ] {
            assert!(Config::validate("mirror", good).is_ok(), "{good}");
        }
    }

    #[test]
    fn validation() {
        assert!(Config::validate("channel", "stable").is_ok());
        assert_eq!(
            Config::validate("channel", "x").unwrap_err().code(),
            "E1870"
        );
        assert!(Config::validate("keep_days", "30").is_ok());
        assert!(Config::validate("keep_days", "soon").is_err());
        for bad in ["0", "3651", "-1", "", "4294967296"] {
            let e = Config::validate("keep_days", bad).unwrap_err();
            assert_eq!(e.code(), "E1870");
            assert!(e.to_string().contains("from 1 to 3650"), "{e}");
        }
        assert!(Config::validate("keep_days", "1").is_ok());
        assert!(Config::validate("keep_days", "3650").is_ok());
        for good in ["1", "10", "10%", "100%", " 50 % "] {
            assert!(Config::validate("rollout", good).is_ok(), "{good}");
        }
        for bad in ["0", "101", "ten", "", "10.5", "-1"] {
            let e = Config::validate("rollout", bad).unwrap_err();
            assert_eq!(e.code(), "E1870");
            assert!(e.to_string().contains("from 1 to 100"), "{e}");
        }
        assert_eq!(Config::from_text("", &no_env).get("rollout"), "100");
        assert!(Config::validate("bogus", "1").is_err());
        assert!(Config::validate("mirror", "https://x").is_ok());
    }
}
