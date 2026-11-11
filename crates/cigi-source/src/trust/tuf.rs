// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `TufTrust`: the manifest and the builds it names are admitted only through TUF metadata that
//! verified against a root compiled into this binary (decision 0004).
//!
//! Order of events in `admit_manifest`: the root chain is walked forward from the compiled-in
//! root, then timestamp, snapshot, targets and the delegated `releases` role are fetched and each
//! checked for signature threshold, expiry and version rollback (by `tough`, against a datastore
//! that remembers the newest version it has seen); then this file adds the compiled-in version
//! floor, and checks the manifest's length and SHA-256 against its signed target. Every error from
//! `tough` is a refusal; nothing here branches on its variant.

use super::{seal, Admitted, Attest, GhAttest, SeenState, Trust, TrustLevel, Verified};
use crate::clock;
use crate::digest::sha256_file;
use crate::error::TrustError;
use crate::pins;
use crate::tuf_transport::UreqTransport;
use cig_compat::{Artifact, Manifest};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tough::schema::{Signed, Targets};
use tough::{
    async_trait, ExpirationEnforcement, RepositoryLoader, TargetName, Transport, TransportError,
    TransportStream,
};
use url::Url;

/// The compiled-in root (`crates/cigi-source/tuf/root.json`). `None` while the file is still the
/// placeholder, so a build without a real root refuses everything instead of trusting anything.
pub fn compiled_root() -> Option<&'static [u8]> {
    static ROOT: &[u8] = include_bytes!("../../tuf/root.json");
    let v: serde_json::Value = serde_json::from_slice(ROOT).ok()?;
    (v.get("signed").is_some() && v.get("signatures").is_some()).then_some(ROOT)
}

/// Everything `TufTrust` needs to know about where its metadata lives and what it remembers.
#[derive(Debug, Clone)]
pub struct TufConfig {
    /// The root this installer shipped with. A newer root is followed from it, never the reverse.
    pub root: Vec<u8>,
    /// A directory URL holding `N.root.json`, `timestamp.json`, `N.snapshot.json`, `N.targets.json`
    /// and `N.releases.json` (the `tuf` rolling release, section 7).
    pub metadata_url: Url,
    /// Where the newest version of each role is remembered (`installer/tuf/<channel>`, 0700).
    /// Deleting it resets the rollback defense, and `doctor` says so.
    pub datastore: PathBuf,
    /// The compiled-in floor: no `snapshot` or `targets` below these is ever accepted.
    pub min_snapshot_version: u64,
    pub min_targets_version: u64,
}

impl TufConfig {
    /// The shipping configuration: the compiled-in root and floor. `None` while the root is a
    /// placeholder.
    pub fn compiled(metadata_url: Url, datastore: PathBuf) -> Option<TufConfig> {
        Some(TufConfig {
            root: compiled_root()?.to_vec(),
            metadata_url,
            datastore,
            min_snapshot_version: crate::tuf_floor::MIN_SNAPSHOT_VERSION,
            min_targets_version: crate::tuf_floor::MIN_TARGETS_VERSION,
        })
    }
}

#[derive(Debug, Clone)]
struct Shared(Arc<dyn Transport>);

#[async_trait]
impl Transport for Shared {
    async fn fetch(&self, url: Url) -> Result<TransportStream, TransportError> {
        self.0.fetch(url).await
    }
}

/// What the signed metadata says about one file.
#[derive(Debug, Clone)]
struct SignedEntry {
    length: u64,
    sha256: String,
    repo: Option<String>,
}

pub struct TufTrust {
    cfg: TufConfig,
    /// The lowest level an artifact may reach (`trust` in config); `Signed` by default.
    pub floor: TrustLevel,
    pub pins: &'static [(&'static str, &'static str)],
    pub attest: Arc<dyn Attest>,
    transport: Shared,
    /// The verified targets of the last admitted manifest; artifacts are looked up here.
    targets: Mutex<Option<Signed<Targets>>>,
}

impl TufTrust {
    pub fn new(cfg: TufConfig) -> TufTrust {
        TufTrust {
            cfg,
            floor: TrustLevel::Signed,
            pins: pins::PINS,
            attest: Arc::new(GhAttest),
            transport: Shared(Arc::new(UreqTransport::new(4 * 1024 * 1024))),
            targets: Mutex::new(None),
        }
    }
    pub fn with_transport<T: Transport + 'static>(mut self, t: T) -> Self {
        self.transport = Shared(Arc::new(t));
        self
    }
    pub fn with_floor(mut self, floor: TrustLevel) -> Self {
        self.floor = floor;
        self
    }
    pub fn with_pins(mut self, pins: &'static [(&'static str, &'static str)]) -> Self {
        self.pins = pins;
        self
    }
    pub fn with_attest(mut self, a: Arc<dyn Attest>) -> Self {
        self.attest = a;
        self
    }

    fn refuse(why: impl std::fmt::Display) -> TrustError {
        TrustError::Tuf {
            why: why.to_string(),
        }
    }

    /// Load and verify the metadata. Online runs always enforce every expiry; `offline` waives
    /// those of timestamp, snapshot and the delegated role (the installer checks root and targets
    /// itself, below) and says so.
    fn load(&self, offline: bool, now: i64) -> Result<(Signed<Targets>, Vec<String>), TrustError> {
        std::fs::create_dir_all(&self.cfg.datastore)
            .map_err(|e| Self::refuse(format!("the metadata datastore: {e}")))?;
        private_dir(&self.cfg.datastore);
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .map_err(|e| Self::refuse(format!("no runtime: {e}")))?;
        let enforcement = if offline {
            ExpirationEnforcement::Unsafe
        } else {
            ExpirationEnforcement::Safe
        };
        let repo = rt
            .block_on(async {
                RepositoryLoader::new(
                    &self.cfg.root,
                    self.cfg.metadata_url.clone(),
                    self.cfg.metadata_url.clone(),
                )
                .transport(self.transport.clone())
                .datastore(&self.cfg.datastore)
                .expiration_enforcement(enforcement)
                .load()
                .await
            })
            .map_err(Self::refuse)?;
        let snap = repo.snapshot().signed.version.get();
        if snap < self.cfg.min_snapshot_version {
            return Err(Self::refuse(format!(
                "snapshot version {snap} is below the {} this installer was built to require",
                self.cfg.min_snapshot_version
            )));
        }
        let tv = repo.targets().signed.version.get();
        if tv < self.cfg.min_targets_version {
            return Err(Self::refuse(format!(
                "targets version {tv} is below the {} this installer was built to require",
                self.cfg.min_targets_version
            )));
        }
        let mut notes = Vec::new();
        if offline {
            for (role, exp) in [
                ("root", repo.root().signed.expires),
                ("targets", repo.targets().signed.expires),
            ] {
                if exp.as_second() <= now {
                    return Err(Self::refuse(format!(
                        "the {role} metadata expired on {exp}; offline use waives the timestamp, snapshot and release expiries, never this one"
                    )));
                }
            }
            let waived: Vec<String> = [
                ("timestamp", repo.timestamp().signed.expires),
                ("snapshot", repo.snapshot().signed.expires),
            ]
            .into_iter()
            .filter(|(_, e)| e.as_second() <= now)
            .map(|(r, e)| format!("{r} (expired {e})"))
            .collect();
            if !waived.is_empty() {
                notes.push(format!(
                    "signed metadata used offline with expired {}",
                    waived.join(", ")
                ));
            }
        }
        Ok((repo.targets().clone(), notes))
    }

    /// The length and SHA-256 the signed metadata vouches for under `name`, after a manifest was
    /// admitted. `ciginstaller verify-script <file>` compares a script with this.
    pub fn signed_digest(&self, name: &str) -> Result<(u64, String), TrustError> {
        let guard = self.targets.lock().unwrap_or_else(|p| p.into_inner());
        let Some(t) = guard.as_ref() else {
            return Err(Self::refuse("no signed metadata has been admitted yet"));
        };
        let e = Self::entry(t, name)?;
        Ok((e.length, e.sha256))
    }

    fn entry(targets: &Signed<Targets>, name: &str) -> Result<SignedEntry, TrustError> {
        let name = TargetName::new(name).map_err(Self::refuse)?;
        let t = targets
            .signed
            .find_target(&name, false)
            .map_err(|_| Self::refuse(format!("{} is not in the signed metadata", name.raw())))?;
        Ok(SignedEntry {
            length: t.length,
            sha256: hex::encode(&t.hashes.sha256),
            repo: t
                .custom
                .get("repo")
                .and_then(|v| v.as_str())
                .map(str::to_string),
        })
    }
}

#[cfg(unix)]
fn private_dir(p: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700));
}
#[cfg(not(unix))]
fn private_dir(_: &Path) {}

impl Trust for TufTrust {
    fn level(&self) -> &'static str {
        "tuf"
    }

    fn admit_manifest(
        &self,
        doc: &[u8],
        _sidecar: Option<&str>,
        _attestation: Option<&[u8]>,
        seen: &SeenState,
    ) -> Result<Admitted<Manifest>, TrustError> {
        let now = seen.now.unwrap_or_else(clock::now);
        let text =
            std::str::from_utf8(doc).map_err(|_| TrustError::Malformed("not UTF-8".into()))?;
        let m = Manifest::from_str(text).map_err(|e| TrustError::Malformed(e.to_string()))?;
        if let Some(c) = &seen.channel {
            if &m.channel != c {
                return Err(TrustError::Malformed(format!(
                    "it is the {:?} channel's manifest, not {c:?}",
                    m.channel
                )));
            }
        }
        let (targets, mut notes) = self.load(seen.offline, now)?;
        let name = format!("manifest-{}.toml", m.channel);
        let e = Self::entry(&targets, &name)?;
        if doc.len() as u64 != e.length {
            return Err(TrustError::SizeMismatch {
                what: name,
                expected: e.length,
                actual: doc.len() as u64,
            });
        }
        let have = crate::digest::sha256_hex(doc);
        if have != e.sha256 {
            return Err(TrustError::DigestMismatch {
                what: name,
                expected: e.sha256,
                actual: have,
            });
        }
        // the manifest's own sequence is a second, independent rollback counter
        if let Some(s) = seen.sequence {
            if m.sequence < s {
                return Err(TrustError::SequenceBackwards {
                    seen: s,
                    got: m.sequence,
                });
            }
        }
        match clock::parse(&m.expires) {
            Some(exp) if exp > now => {}
            Some(_) if seen.offline => notes.push(format!(
                "the manifest expired on {}; used because you are offline",
                m.expires
            )),
            Some(_) => {
                return Err(TrustError::Expired {
                    expires: m.expires.clone(),
                })
            }
            None => {
                return Err(TrustError::Malformed(format!(
                    "expires is not a date: {:?}",
                    m.expires
                )))
            }
        }
        *self.targets.lock().unwrap_or_else(|p| p.into_inner()) = Some(targets);
        let mut a = seal(m, self).note("manifest verified against signed metadata");
        for n in notes {
            a = a.note(n);
        }
        Ok(a)
    }

    fn admit_artifact(
        &self,
        art: &Artifact,
        _sidecar: Option<&str>,
        file: &Path,
    ) -> Result<Admitted<Verified>, TrustError> {
        let guard = self.targets.lock().unwrap_or_else(|p| p.into_inner());
        let Some(targets) = guard.as_ref() else {
            return Err(Self::refuse(
                "no signed metadata has been admitted yet; the manifest comes first",
            ));
        };
        let name = pins::key(&art.tool, &art.version.to_string(), &art.target);
        let e = Self::entry(targets, &name)?;
        let io = |er: std::io::Error| TrustError::Io {
            what: art.asset.clone(),
            why: er.to_string(),
        };
        let len = std::fs::metadata(file).map_err(io)?.len();
        if len != e.length {
            return Err(TrustError::SizeMismatch {
                what: art.asset.clone(),
                expected: e.length,
                actual: len,
            });
        }
        let (actual, _) = sha256_file(file).map_err(io)?;
        if actual != e.sha256 {
            return Err(TrustError::DigestMismatch {
                what: art.asset.clone(),
                expected: e.sha256,
                actual,
            });
        }
        // the manifest and the signed entry must tell the same story
        if actual != art.sha256 || (art.size != 0 && art.size != e.length) {
            return Err(Self::refuse(format!(
                "{} is signed with a digest or size the manifest does not name",
                art.asset
            )));
        }
        // a compiled-in pin that disagrees with a signed digest is a refusal, not a downgrade
        let key = &name;
        let pinned = match self.pins.iter().find(|(k, _)| k == key) {
            Some((_, p)) if *p == actual => true,
            Some((_, p)) => {
                return Err(TrustError::DigestMismatch {
                    what: format!("{} (compiled-in pin)", art.asset),
                    expected: (*p).to_string(),
                    actual,
                })
            }
            None => false,
        };
        // the attestation reference is signed; it must name the repository the manifest does
        let mut attested_by = None;
        if art.attests {
            match &e.repo {
                Some(r) if *r == art.repo => {}
                other => {
                    return Err(TrustError::AttestationFailed {
                        what: art.asset.clone(),
                        why: format!(
                            "the signed entry names {:?} as the build's repository, the manifest names {:?}",
                            other, art.repo
                        ),
                    })
                }
            }
            if self.attest.available() {
                if !pins::attest_repo_allowed(&art.repo) {
                    return Err(TrustError::AttestationFailed {
                        what: art.asset.clone(),
                        why: format!(
                            "{} is not a repository this installer accepts attestations from",
                            art.repo
                        ),
                    });
                }
                attested_by = Some(self.attest.verify(file, &art.repo, None).map_err(|why| {
                    TrustError::AttestationFailed {
                        what: art.asset.clone(),
                        why,
                    }
                })?);
            }
        }
        let level = if pinned {
            TrustLevel::Pinned
        } else {
            TrustLevel::Signed
        };
        if level < self.floor {
            return Err(TrustError::FloorNotMet {
                floor: self.floor.as_str(),
                reached: level.as_str(),
                why: format!("{} is not in this installer's compiled-in pins", art.asset),
            });
        }
        Ok(seal(
            Verified {
                level,
                sha256: actual,
                independent: true,
                attested_by,
            },
            self,
        ))
    }
}
