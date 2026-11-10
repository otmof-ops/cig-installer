// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! The one HTTP client (ureq, rustls). It sends `GET` with `User-Agent: ciginstaller` and nothing
//! else of its own: no cookies, no query string, no identifiers (#19). Redirects are followed by hand
//! so the host rules are ours: same host always, plus a fixed allow-list for GitHub's release CDN;
//! never https to http; at most five hops.

use crate::error::FetchError;
use crate::progress::Progress;
use crate::source::FetchInfo;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;
use url::Url;

pub const USER_AGENT: &str = "ciginstaller";
/// Hard ceiling for one download (section 6.2 keeps every build below 1 GB).
pub const MAX_DOWNLOAD: u64 = 1024 * 1024 * 1024;
const MAX_HOPS: usize = 5;

#[derive(Debug, Clone)]
pub struct HttpConfig {
    pub connect_timeout: Duration,
    /// Per read: a body that stalls this long is abandoned (section 14.2: 30 s).
    pub read_timeout: Duration,
    /// The whole request, body included: a server that drips bytes cannot hold a download open.
    pub total_timeout: Duration,
    pub redirects: RedirectPolicy,
    pub max_bytes: u64,
}
impl Default for HttpConfig {
    fn default() -> Self {
        HttpConfig {
            connect_timeout: Duration::from_secs(10),
            read_timeout: Duration::from_secs(30),
            total_timeout: Duration::from_secs(30 * 60),
            redirects: RedirectPolicy::SameHost,
            max_bytes: MAX_DOWNLOAD,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedirectPolicy {
    /// Only to the host (and port) the request started at.
    SameHost,
    /// Same host, or one of `GITHUB_HOSTS` (github.com and the release-asset hosts).
    Github,
}

fn host_key(u: &Url) -> String {
    format!(
        "{}:{}",
        u.host_str().unwrap_or_default().to_ascii_lowercase(),
        u.port_or_known_default().unwrap_or(0)
    )
}

/// The hosts a GitHub release download may live on or be redirected to.
pub const GITHUB_HOSTS: &[&str] = &[
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
    "github-releases.githubusercontent.com",
];

pub fn github_host(u: &Url) -> bool {
    let h = u.host_str().unwrap_or_default().to_ascii_lowercase();
    u.scheme() == "https"
        && u.port_or_known_default() == Some(443)
        && GITHUB_HOSTS.contains(&h.as_str())
}

pub fn is_loopback(u: &Url) -> bool {
    match u.host() {
        Some(url::Host::Domain(_)) => false,
        Some(url::Host::Ipv4(a)) => a.is_loopback(),
        Some(url::Host::Ipv6(a)) => a.is_loopback(),
        None => false,
    }
}

/// May this URL be requested at all? https anywhere; http only to loopback.
pub fn check_scheme(u: &Url) -> Result<(), FetchError> {
    match u.scheme() {
        "https" => Ok(()),
        "http" if is_loopback(u) => Ok(()),
        "http" => Err(FetchError::Refused {
            url: u.to_string(),
            why: "plain http is only allowed to a loopback address".into(),
        }),
        s => Err(FetchError::Refused {
            url: u.to_string(),
            why: format!("the {s} scheme is not supported here"),
        }),
    }
}

fn net(url: &Url, why: impl ToString) -> FetchError {
    FetchError::Network {
        url: url.to_string(),
        why: why.to_string(),
    }
}

/// Issue the request, following redirects under the policy; returns the final response.
fn open(url: &Url, cfg: &HttpConfig) -> Result<ureq::Response, FetchError> {
    check_scheme(url)?;
    if url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(FetchError::Refused {
            url: url.to_string(),
            why: "a request carries no query, fragment or credentials".into(),
        });
    }
    let agent = ureq::AgentBuilder::new()
        .user_agent(USER_AGENT)
        .redirects(0)
        .timeout_connect(cfg.connect_timeout)
        .timeout_read(cfg.read_timeout)
        .build();
    let origin = host_key(url);
    let mut cur = url.clone();
    for _ in 0..=MAX_HOPS {
        let resp = match agent.get(cur.as_str()).call() {
            Ok(r) => r,
            Err(ureq::Error::Status(404, _)) | Err(ureq::Error::Status(410, _)) => {
                return Err(FetchError::NotFound {
                    url: cur.to_string(),
                })
            }
            Err(ureq::Error::Status(code, _)) => {
                return Err(net(&cur, format!("the server answered {code}")))
            }
            Err(e) => return Err(net(&cur, e)),
        };
        if !matches!(resp.status(), 301 | 302 | 303 | 307 | 308) {
            return Ok(resp);
        }
        let Some(loc) = resp.header("location") else {
            return Err(net(&cur, "a redirect without a Location"));
        };
        let next = cur
            .join(loc)
            .map_err(|e| net(&cur, format!("bad redirect target: {e}")))?;
        check_scheme(&next)?;
        if cur.scheme() == "https" && next.scheme() != "https" {
            return Err(FetchError::Refused {
                url: next.to_string(),
                why: "redirect from https to http".into(),
            });
        }
        let same = host_key(&next) == origin;
        let allowed = same || (cfg.redirects == RedirectPolicy::Github && github_host(&next));
        if !allowed {
            return Err(FetchError::Refused {
                url: next.to_string(),
                why: format!(
                    "redirect to another host ({}) is not allowed",
                    next.host_str().unwrap_or_default()
                ),
            });
        }
        cur = next;
    }
    Err(net(url, "too many redirects"))
}

/// A new file that must not exist yet (no symlink or planted file is followed), owner-only.
pub fn create_private(path: &Path) -> std::io::Result<std::fs::File> {
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(path)
}

/// GET into `dest` (created new, owner-only), hashing as it goes.
pub fn get_to_file(
    url: &Url,
    cfg: &HttpConfig,
    dest: &Path,
    progress: &mut dyn Progress,
) -> Result<FetchInfo, FetchError> {
    let started = std::time::Instant::now();
    let resp = open(url, cfg)?;
    let declared = resp
        .header("content-length")
        .and_then(|v| v.trim().parse::<u64>().ok());
    if let Some(n) = declared {
        if n > cfg.max_bytes {
            return Err(FetchError::TooLarge {
                url: url.to_string(),
                limit: cfg.max_bytes,
            });
        }
        progress.set_total(n);
    }
    let io = |source| FetchError::Io {
        path: dest.to_path_buf(),
        source,
    };
    let mut out = create_private(dest).map_err(io)?;
    let mut reader = resp.into_reader().take(cfg.max_bytes + 1);
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| net(url, format!("the body stopped early: {e}")))?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if started.elapsed() > cfg.total_timeout {
            return Err(net(
                url,
                format!("the download took longer than {:?}", cfg.total_timeout),
            ));
        }
        if total > cfg.max_bytes {
            return Err(FetchError::TooLarge {
                url: url.to_string(),
                limit: cfg.max_bytes,
            });
        }
        hash.update(&buf[..n]);
        out.write_all(&buf[..n]).map_err(io)?;
        progress.advance(n as u64);
    }
    out.sync_all().map_err(io)?;
    if let Some(n) = declared {
        if n != total {
            return Err(net(
                url,
                format!("the body ended after {total} of {n} bytes"),
            ));
        }
    }
    Ok(FetchInfo {
        bytes: total,
        sha256: hex::encode(hash.finalize()),
    })
}

/// GET as text, at most `max_bytes`.
pub fn get_text(url: &Url, cfg: &HttpConfig, max_bytes: usize) -> Result<String, FetchError> {
    let started = std::time::Instant::now();
    let resp = open(url, cfg)?;
    let mut body = Vec::new();
    let mut reader = resp.into_reader().take(max_bytes as u64 + 1);
    let mut buf = [0u8; 8192];
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| net(url, format!("the body stopped early: {e}")))?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&buf[..n]);
        if started.elapsed() > cfg.total_timeout {
            return Err(net(
                url,
                format!("the download took longer than {:?}", cfg.total_timeout),
            ));
        }
    }
    if body.len() > max_bytes {
        return Err(FetchError::TooLarge {
            url: url.to_string(),
            limit: max_bytes as u64,
        });
    }
    String::from_utf8(body).map_err(|_| net(url, "the body is not UTF-8 text"))
}
