// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! Sources: where a `Location` lives. GitHub release URLs, a mirror (https, `file://`, or loopback
//! http), and `InvalidSource` for a configuration that could not be understood (it fails at first use,
//! loudly, instead of silently falling back to GitHub).

use crate::error::FetchError;
use crate::http::{self, HttpConfig, RedirectPolicy};
use crate::location::{is_plain_repo, is_plain_segment, Location};
use crate::progress::Progress;
use cigi_home::Config;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use url::Url;

/// What a completed `fetch` knows about the bytes it wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchInfo {
    pub bytes: u64,
    /// sha256 of what was written, lowercase hex. Informational: trust recomputes its own.
    pub sha256: String,
}

pub trait Source: Send + Sync {
    /// `"github"`, `"mirror:https://..."`, `"dir:/path"`.
    fn name(&self) -> String;
    /// true for GitHub (manifest and product sidecars come from different repositories),
    /// false for a mirror or a directory (one operator serves both).
    fn independent(&self) -> bool;
    /// Does this source carry its files over plain http (a loopback mirror)? Then only a build
    /// a pin vouches for is accepted, whatever `allow_single_source` says (E1900).
    fn plain_http(&self) -> bool {
        false
    }
    fn fetch(
        &self,
        loc: &Location,
        dest: &Path,
        progress: &mut dyn Progress,
    ) -> Result<FetchInfo, FetchError>;
    fn fetch_text(&self, loc: &Location, max_bytes: usize) -> Result<String, FetchError>;
}

// ---------------------------------------------------------------- GitHub

#[derive(Debug, Clone, Copy, Default)]
pub struct GithubSource;

impl GithubSource {
    pub fn url_for(loc: &Location) -> Result<Url, FetchError> {
        let text = match loc {
            Location::Url { url } => url.clone(),
            other => {
                let (repo, tag) = other.github_parts().unwrap_or_default();
                let name = other.file_name();
                if !is_plain_repo(&repo) || !is_plain_segment(&tag) || !is_plain_segment(&name) {
                    return Err(FetchError::Refused {
                        url: format!("{repo}/{tag}/{name}"),
                        why: "repository, tag or file name has characters a release URL never has"
                            .into(),
                    });
                }
                format!("https://github.com/{repo}/releases/download/{tag}/{name}")
            }
        };
        let u = Url::parse(&text).map_err(|e| FetchError::Refused {
            url: text.clone(),
            why: e.to_string(),
        })?;
        if u.scheme() != "https" {
            return Err(FetchError::Refused {
                url: text,
                why: "GitHub downloads are https only".into(),
            });
        }
        // a manifest `url` may not take the download off GitHub's release hosts
        if !http::github_host(&u) {
            return Err(FetchError::Refused {
                url: text,
                why: "a build's url must be on github.com or a GitHub release-asset host; \
                      anywhere else needs a declared mirror"
                    .into(),
            });
        }
        Ok(u)
    }
    fn cfg() -> HttpConfig {
        HttpConfig {
            redirects: RedirectPolicy::Github,
            ..HttpConfig::default()
        }
    }
}

impl Source for GithubSource {
    fn name(&self) -> String {
        "github".into()
    }
    fn independent(&self) -> bool {
        true
    }
    fn fetch(
        &self,
        loc: &Location,
        dest: &Path,
        progress: &mut dyn Progress,
    ) -> Result<FetchInfo, FetchError> {
        let r = http::get_to_file(&Self::url_for(loc)?, &Self::cfg(), dest, progress);
        progress.done();
        r
    }
    fn fetch_text(&self, loc: &Location, max_bytes: usize) -> Result<String, FetchError> {
        http::get_text(&Self::url_for(loc)?, &Self::cfg(), max_bytes)
    }
}

// ---------------------------------------------------------------- mirror

/// A flat directory of release files: `manifest-<channel>.toml` and every asset by its file name.
/// `base` is `https://host/path/`, `file:///path/`, or `http://127.0.0.1:PORT/` (loopback only).
#[derive(Debug, Clone)]
pub struct MirrorSource {
    pub base: Url,
}

impl MirrorSource {
    /// Checked constructor: scheme rules and no query/fragment/credentials.
    pub fn new(base: Url) -> Result<MirrorSource, FetchError> {
        let bad = |why: &str| FetchError::Refused {
            url: base.to_string(),
            why: why.into(),
        };
        if base.query().is_some()
            || base.fragment().is_some()
            || !base.username().is_empty()
            || base.password().is_some()
        {
            return Err(bad(
                "a mirror URL carries no query, fragment or credentials",
            ));
        }
        match base.scheme() {
            "file" => {
                base.to_file_path()
                    .map_err(|_| bad("not a local file URL"))?;
            }
            _ => http::check_scheme(&base)?,
        }
        Ok(MirrorSource { base })
    }

    /// Accepts `https://...`, `http://127.0.0.1:N`, `file:///dir`, or a plain absolute path.
    pub fn parse(s: &str) -> Result<MirrorSource, FetchError> {
        let s = s.trim();
        let url = if s.contains("://") {
            Url::parse(s).map_err(|e| FetchError::Refused {
                url: s.into(),
                why: e.to_string(),
            })?
        } else if Path::new(s).is_absolute() {
            Url::from_directory_path(s).map_err(|()| FetchError::Refused {
                url: s.into(),
                why: "not a usable directory path".into(),
            })?
        } else {
            return Err(FetchError::Refused {
                url: s.into(),
                why: "a mirror is an https URL, a file:// URL, or an absolute directory".into(),
            });
        };
        MirrorSource::new(url)
    }

    fn is_file(&self) -> bool {
        self.base.scheme() == "file"
    }

    fn dir(&self) -> Result<PathBuf, FetchError> {
        self.base.to_file_path().map_err(|()| FetchError::Refused {
            url: self.base.to_string(),
            why: "not a local file URL".into(),
        })
    }

    fn file_path(&self, loc: &Location) -> Result<PathBuf, FetchError> {
        let name = loc.file_name();
        if !is_plain_segment(&name) {
            return Err(FetchError::Refused {
                url: name,
                why: "not a plain file name".into(),
            });
        }
        Ok(self.dir()?.join(name))
    }

    fn http_url(&self, loc: &Location) -> Result<Url, FetchError> {
        let name = loc.file_name();
        if !is_plain_segment(&name) {
            return Err(FetchError::Refused {
                url: name,
                why: "not a plain file name".into(),
            });
        }
        let mut base = self.base.clone();
        if !base.path().ends_with('/') {
            let p = format!("{}/", base.path());
            base.set_path(&p);
        }
        base.join(&name).map_err(|e| FetchError::Refused {
            url: name,
            why: e.to_string(),
        })
    }
}

impl Source for MirrorSource {
    fn name(&self) -> String {
        if self.is_file() {
            format!(
                "dir:{}",
                self.dir()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default()
            )
        } else {
            format!("mirror:{}", self.base)
        }
    }
    fn independent(&self) -> bool {
        false
    }
    fn plain_http(&self) -> bool {
        self.base.scheme() == "http"
    }
    fn fetch(
        &self,
        loc: &Location,
        dest: &Path,
        progress: &mut dyn Progress,
    ) -> Result<FetchInfo, FetchError> {
        let r = if self.is_file() {
            copy_file(&self.file_path(loc)?, dest, progress)
        } else {
            http::get_to_file(&self.http_url(loc)?, &HttpConfig::default(), dest, progress)
        };
        progress.done();
        r
    }
    fn fetch_text(&self, loc: &Location, max_bytes: usize) -> Result<String, FetchError> {
        if self.is_file() {
            let p = self.file_path(loc)?;
            let mut f = open_or_not_found(&p)?;
            let mut body = Vec::new();
            Read::by_ref(&mut f)
                .take(max_bytes as u64 + 1)
                .read_to_end(&mut body)
                .map_err(|source| FetchError::Io {
                    path: p.clone(),
                    source,
                })?;
            if body.len() > max_bytes {
                return Err(FetchError::TooLarge {
                    url: p.display().to_string(),
                    limit: max_bytes as u64,
                });
            }
            String::from_utf8(body).map_err(|_| FetchError::Network {
                url: p.display().to_string(),
                why: "not UTF-8 text".into(),
            })
        } else {
            http::get_text(&self.http_url(loc)?, &HttpConfig::default(), max_bytes)
        }
    }
}

fn open_or_not_found(p: &Path) -> Result<std::fs::File, FetchError> {
    std::fs::File::open(p).map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            FetchError::NotFound {
                url: p.display().to_string(),
            }
        } else {
            FetchError::Io {
                path: p.to_path_buf(),
                source,
            }
        }
    })
}

fn copy_file(
    from: &Path,
    dest: &Path,
    progress: &mut dyn Progress,
) -> Result<FetchInfo, FetchError> {
    let mut src = open_or_not_found(from)?;
    let meta = src.metadata().map_err(|source| FetchError::Io {
        path: from.to_path_buf(),
        source,
    })?;
    if !meta.is_file() {
        return Err(FetchError::NotFound {
            url: from.display().to_string(),
        });
    }
    if meta.len() > http::MAX_DOWNLOAD {
        return Err(FetchError::TooLarge {
            url: from.display().to_string(),
            limit: http::MAX_DOWNLOAD,
        });
    }
    progress.set_total(meta.len());
    let io = |source| FetchError::Io {
        path: dest.to_path_buf(),
        source,
    };
    let mut out = http::create_private(dest).map_err(io)?;
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = src.read(&mut buf).map_err(|source| FetchError::Io {
            path: from.to_path_buf(),
            source,
        })?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
        out.write_all(&buf[..n]).map_err(io)?;
        total += n as u64;
        progress.advance(n as u64);
    }
    out.sync_all().map_err(io)?;
    Ok(FetchInfo {
        bytes: total,
        sha256: hex::encode(hash.finalize()),
    })
}

// ---------------------------------------------------------------- bad configuration

/// A source whose configuration was unusable. Every call fails with the reason.
#[derive(Debug, Clone)]
pub struct InvalidSource {
    pub reason: String,
}
impl Source for InvalidSource {
    fn name(&self) -> String {
        "invalid".into()
    }
    fn independent(&self) -> bool {
        false
    }
    fn fetch(
        &self,
        loc: &Location,
        _dest: &Path,
        _p: &mut dyn Progress,
    ) -> Result<FetchInfo, FetchError> {
        Err(FetchError::Refused {
            url: loc.file_name(),
            why: self.reason.clone(),
        })
    }
    fn fetch_text(&self, loc: &Location, _max: usize) -> Result<String, FetchError> {
        Err(FetchError::Refused {
            url: loc.file_name(),
            why: self.reason.clone(),
        })
    }
}

/// The source the config names: `mirror` set means a mirror, unset means GitHub.
pub fn try_source_from_config(cfg: &Config) -> Result<Box<dyn Source>, FetchError> {
    let m = cfg.get("mirror");
    if m.trim().is_empty() {
        Ok(Box::new(GithubSource))
    } else {
        Ok(Box::new(MirrorSource::parse(&m)?))
    }
}

/// Section 2.3's signature. An unusable `mirror` value yields a source that refuses every request
/// with the reason (never a silent fall back to GitHub: someone who set a mirror did not ask for GitHub).
pub fn source_from_config(cfg: &Config) -> Box<dyn Source> {
    match try_source_from_config(cfg) {
        Ok(s) => s,
        Err(e) => Box::new(InvalidSource {
            reason: e.to_string(),
        }),
    }
}
