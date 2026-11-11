// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! What travels, and the one request that carries it (#28 part 1, #19).
//!
//! `travel` is the allowlist: the object `crash show` prints and `crash send` sends. Its keys are
//! read from `schemas/report.schema.json`, so the schema is the one place the allowlist lives.
//! The request is a single `POST` of that object to the issues route: no query string, no
//! credentials, no cookies, no redirects followed, `User-Agent: ciginstaller` and nothing else
//! of its own. Nothing here identifies the person or the machine.

use std::collections::HashSet;
use std::io::Read;
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

use serde_json::{json, Map, Value};
use url::Url;

use super::{anon, Report};

/// The published schema, built in: the allowlist is read from it.
pub const SCHEMA: &str = include_str!("../../../../schemas/report.schema.json");

/// The environment variable that names the issues route: an `https://` URL (`http://` to a
/// loopback address only). With none set there is no route yet.
pub const ROUTE_ENV: &str = "CIGI_REPORT_ROUTE";

/// A report cut back to this many bytes when it would be larger: the repro goes first.
const BODY_MAX: usize = 60_000;

fn schema() -> &'static Value {
    static S: OnceLock<Value> = OnceLock::new();
    S.get_or_init(|| serde_json::from_str(SCHEMA).unwrap_or(Value::Null))
}

/// The keys a report may carry.
pub fn allowed_keys() -> HashSet<String> {
    schema()["properties"]
        .as_object()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default()
}

fn plain(s: &str, max: usize, extra: &[char]) -> String {
    s.chars()
        .filter(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '+') || extra.contains(c)
        })
        .take(max)
        .collect()
}

/// The object that would travel for `r`: its fields through the allowlist, every string
/// anonymised again (a file from another product, or one edited by hand, gets no benefit of the
/// doubt), the ashtray's own bookkeeping left at home.
pub fn travel(r: &Report, root: &Path) -> Value {
    let home = Some(root);
    let scrub = |s: &str| anon::scrub_text(s, home);
    let mut m = Map::new();
    m.insert("id".into(), json!(plain(&r.id, 64, &[])));
    m.insert("fingerprint".into(), json!(plain(&r.fingerprint, 8, &[])));
    m.insert("kind".into(), json!(r.kind.word()));
    m.insert("origin".into(), json!(plain(&r.origin, 40, &[])));
    m.insert("version".into(), json!(plain(&r.version, 40, &[])));
    m.insert("os".into(), json!(plain(&r.os, 40, &[])));
    m.insert("arch".into(), json!(plain(&r.arch, 40, &[])));
    m.insert("timestamp".into(), json!(plain(&r.timestamp, 40, &[':'])));
    m.insert(
        "command".into(),
        json!(r.command.iter().map(|c| scrub(c)).collect::<Vec<_>>()),
    );
    m.insert("cwd".into(), json!(scrub(&r.cwd)));
    m.insert(
        "panic_message".into(),
        json!(super::truncate(
            &anon::anonymise_text_in(&r.panic_message, home),
            500
        )),
    );
    m.insert("panic_location".into(), json!(scrub(&r.panic_location)));
    m.insert(
        "backtrace".into(),
        json!(r
            .backtrace
            .iter()
            .take(40)
            .map(|f| scrub(f))
            .collect::<Vec<_>>()),
    );
    m.insert(
        "active_run_id".into(),
        json!(r.active_run_id.as_deref().map(|s| plain(s, 64, &[]))),
    );
    m.insert("rolled_back".into(), json!(r.rolled_back));
    if let Some(t) = &r.title {
        m.insert(
            "title".into(),
            json!(super::truncate(&anon::anonymise_text_in(t, home), 500)),
        );
    }
    if let Some(d) = &r.details {
        let mut d = d.clone();
        let kept: Vec<String> = r
            .extra
            .get("kept_spans")
            .and_then(|k| serde_json::from_value(k.clone()).ok())
            .unwrap_or_default();
        anon::anonymise_details(&mut d, home, &kept);
        m.insert("details".into(), d);
    }
    if let Some(x) = r.repro.as_ref().filter(|x| x.is_object()) {
        let mut x = x.clone();
        anon::anonymise_value(&mut x, home);
        m.insert("repro".into(), x);
    }
    let allowed = allowed_keys();
    m.retain(|k, _| allowed.contains(k));
    let mut v = Value::Object(m);
    if v.to_string().len() > BODY_MAX {
        if let Some(o) = v.as_object_mut() {
            o.remove("repro");
        }
    }
    if v.to_string().len() > BODY_MAX {
        if let Some(o) = v.as_object_mut() {
            o.insert(
                "details".into(),
                json!({ "note": "cut: too large to send" }),
            );
        }
    }
    v
}

/// The issues route this run is told to use, if any.
pub fn route_from_env() -> Result<Option<Url>, String> {
    let Ok(raw) = std::env::var(ROUTE_ENV) else {
        return Ok(None);
    };
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    let u = Url::parse(raw).map_err(|e| format!("{ROUTE_ENV} is not a URL: {e}"))?;
    cigi_source::http::check_scheme(&u).map_err(|e| format!("{ROUTE_ENV}: {e}"))?;
    if u.query().is_some()
        || u.fragment().is_some()
        || !u.username().is_empty()
        || u.password().is_some()
    {
        return Err(format!(
            "{ROUTE_ENV} carries no query, fragment or credentials"
        ));
    }
    Ok(Some(u))
}

/// Why a send did not file the report.
#[derive(Debug)]
pub struct SendFailure {
    pub why: String,
}

/// `POST` the report. On success, the issue's link when the route answered with one.
pub fn post(route: &Url, body: &Value) -> Result<Option<String>, SendFailure> {
    let fail = |why: String| SendFailure { why };
    let agent = ureq::AgentBuilder::new()
        .tls_config(cigi_source::http::tls_config())
        .user_agent(cigi_source::http::USER_AGENT)
        .redirects(0)
        .timeout_connect(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build();
    let resp = match agent
        .post(route.as_str())
        .set("Content-Type", "application/json")
        .send_string(&body.to_string())
    {
        Ok(r) => r,
        Err(ureq::Error::Status(code, _)) => {
            return Err(fail(format!("the route answered {code}")));
        }
        Err(ureq::Error::Transport(t)) => {
            return Err(fail(format!(
                "the route could not be reached ({})",
                t.kind()
            )));
        }
    };
    if !(200..300).contains(&resp.status()) {
        return Err(fail(format!(
            "the route answered {} (a redirect is not followed: a report goes only where it was told to)",
            resp.status()
        )));
    }
    let mut text = String::new();
    let _ = resp.into_reader().take(65_536).read_to_string(&mut text);
    Ok(serde_json::from_str::<Value>(&text).ok().and_then(|v| {
        ["html_url", "url", "issue_url"]
            .iter()
            .find_map(|k| v[*k].as_str().map(str::to_string))
            .filter(|u| u.starts_with("https://") || u.starts_with("http://"))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_allowlist_is_the_schemas_properties() {
        let keys = allowed_keys();
        for k in [
            "id",
            "fingerprint",
            "kind",
            "origin",
            "version",
            "panic_message",
            "backtrace",
            "details",
            "repro",
        ] {
            assert!(keys.contains(k), "{k}");
        }
        for k in [
            "queue",
            "issue",
            "sent",
            "repo",
            "fixed_in",
            "announced",
            "extra",
        ] {
            assert!(!keys.contains(k), "{k} must stay home");
        }
        let required: Vec<&str> = schema()["required"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert!(required.iter().all(|k| keys.contains(*k)));
    }
}
