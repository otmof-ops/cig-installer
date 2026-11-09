// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0
//! `DigestTrust`: pinned, attested, checksummed (section 7.1), and manifest admission (7.2).

use super::{seal, Admitted, Attest, GhAttest, SeenState, Trust, TrustLevel, Verified};
use crate::clock;
use crate::digest::sha256_file;
use crate::error::TrustError;
use crate::pins;
use crate::sidecar;
use cig_compat::{Artifact, Manifest};
use std::path::Path;
use std::sync::Arc;

/// An online manifest may not claim to be good for longer than this.
pub const MAX_EXPIRES_AHEAD_SECS: i64 = 90 * 24 * 60 * 60;

pub struct DigestTrust {
    /// The lowest level an artifact may reach (`trust` in config).
    pub floor: TrustLevel,
    pub pins: &'static [(&'static str, &'static str)],
    /// How attestations are checked; `GhAttest` by default. Not in section 2.3's struct; added so
    /// tests (and `trust = checksummed` hosts without `gh`) can swap it.
    pub attest: Arc<dyn Attest>,
    /// `allow_single_source` in config: a mirror's `checksummed` build is accepted.
    pub allow_single_source: bool,
}

impl DigestTrust {
    /// The shipping configuration: compiled-in pins, `gh` for attestations.
    pub fn new(floor: TrustLevel) -> DigestTrust {
        DigestTrust {
            floor,
            pins: pins::PINS,
            attest: Arc::new(GhAttest),
            allow_single_source: false,
        }
    }
    pub fn with_pins(mut self, pins: &'static [(&'static str, &'static str)]) -> Self {
        self.pins = pins;
        self
    }
    pub fn with_single_source(mut self, allow: bool) -> Self {
        self.allow_single_source = allow;
        self
    }
    pub fn with_attest(mut self, a: Arc<dyn Attest>) -> Self {
        self.attest = a;
        self
    }

    fn pin_for(&self, art: &Artifact) -> Option<&'static str> {
        let key = pins::key(&art.tool, &art.version.to_string(), &art.target);
        self.pins.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
    }
}

impl Trust for DigestTrust {
    fn level(&self) -> &'static str {
        "digest"
    }

    fn allows_single_source(&self) -> bool {
        self.allow_single_source
    }

    fn admit_manifest(
        &self,
        doc: &[u8],
        sidecar_text: Option<&str>,
        attestation: Option<&[u8]>,
        seen: &SeenState,
    ) -> Result<Admitted<Manifest>, TrustError> {
        let what = "the manifest".to_string();
        // 1. the sidecar names the document's digest
        let Some(sidecar_text) = sidecar_text else {
            return Err(TrustError::SidecarMissing { what });
        };
        let want = sidecar::parse(sidecar_text)
            .map_err(|e| TrustError::Malformed(format!("checksum file: {e}")))?;
        let have = crate::digest::sha256_hex(doc);
        if want != have {
            return Err(TrustError::SidecarDisagrees {
                what,
                manifest: have,
                sidecar: want,
            });
        }
        // 2. parse (E1825)
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
        // 3. rollback protection (equal is fine)
        if let Some(s) = seen.sequence {
            if m.sequence < s {
                return Err(TrustError::SequenceBackwards {
                    seen: s,
                    got: m.sequence,
                });
            }
        }
        // 4. expiry
        let now = seen.now.unwrap_or_else(clock::now);
        let mut notes = Vec::new();
        match clock::parse(&m.expires) {
            Some(exp) if exp > now => {
                if !seen.offline && exp > now + MAX_EXPIRES_AHEAD_SECS {
                    return Err(TrustError::Malformed(format!(
                        "expires ({}) is more than 90 days ahead; a manifest that never goes stale cannot be told from a frozen one",
                        m.expires
                    )));
                }
            }
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
        // 5. attestation: required from `attested` up, a note below it
        let must = self.floor >= TrustLevel::Attested && !seen.offline;
        let not_attested = |why: String| TrustError::AttestationFailed {
            what: "the manifest".into(),
            why,
        };
        match attestation {
            None if must => {
                return Err(not_attested(
                    "no attestation bundle was published with it".into(),
                ))
            }
            None => notes.push("manifest not attested".into()),
            Some(_) if !self.attest.available() && must => {
                return Err(not_attested(
                    "gh is not available to check the attestation".into(),
                ))
            }
            Some(_) if !self.attest.available() => notes.push("manifest not attested".into()),
            Some(bundle) => match verify_manifest_attestation(
                self.attest.as_ref(),
                doc,
                bundle,
                seen.scratch.as_deref(),
            ) {
                Ok(by) => notes.push(format!("attestation verified ({by})")),
                Err(why) if must => return Err(not_attested(why)),
                Err(why) => notes.push(format!("manifest not attested: {why}")),
            },
        }
        let mut a = seal(m, self);
        for n in notes {
            a = a.note(n);
        }
        Ok(a)
    }

    fn admit_artifact(
        &self,
        art: &Artifact,
        sidecar_text: Option<&str>,
        file: &Path,
    ) -> Result<Admitted<Verified>, TrustError> {
        let what = art.asset.clone();
        // size first: cheap, and refuses a truncated or padded file before hashing it
        let io = |e: std::io::Error| TrustError::Io {
            what: what.clone(),
            why: e.to_string(),
        };
        let len = std::fs::metadata(file).map_err(io)?.len();
        if art.size != 0 && len != art.size {
            return Err(TrustError::SizeMismatch {
                what,
                expected: art.size,
                actual: len,
            });
        }
        let (actual, _) = sha256_file(file).map_err(io)?;
        if actual != art.sha256 {
            return Err(TrustError::DigestMismatch {
                what,
                expected: art.sha256.clone(),
                actual,
            });
        }
        // a compiled-in pin for this build must agree with the manifest (a pin that disagrees is an attack or a stale pin)
        let pinned = match self.pin_for(art) {
            Some(p) if p == actual => true,
            Some(p) => {
                return Err(TrustError::DigestMismatch {
                    what: format!("{} (compiled-in pin)", art.asset),
                    expected: p.to_string(),
                    actual,
                })
            }
            None => false,
        };
        // the product's own sidecar
        let side = match sidecar_text {
            Some(t) => {
                let d = sidecar::parse(t).map_err(|e| {
                    TrustError::Malformed(format!("checksum file of {}: {e}", art.asset))
                })?;
                if d != actual {
                    return Err(TrustError::SidecarDisagrees {
                        what: art.asset.clone(),
                        manifest: art.sha256.clone(),
                        sidecar: d,
                    });
                }
                true
            }
            None => false,
        };
        if !side && !pinned {
            return Err(TrustError::SidecarMissing {
                what: art.asset.clone(),
            });
        }
        // attestation (only if asked for by the tool and checkable here)
        let mut attested_by = None;
        let mut attest_fail = None;
        if art.attests && side && self.attest.available() {
            if !crate::pins::attest_repo_allowed(&art.repo) {
                attest_fail = Some(format!(
                    "{} is not a repository this installer accepts attestations from",
                    art.repo
                ));
            } else {
                match self.attest.verify(file, &art.repo, None) {
                    Ok(by) => attested_by = Some(by),
                    Err(why) => attest_fail = Some(why),
                }
            }
        }
        let level = if pinned {
            TrustLevel::Pinned
        } else if attested_by.is_some() {
            TrustLevel::Attested
        } else {
            TrustLevel::Checksummed
        };
        if level < self.floor {
            if let (TrustLevel::Attested, Some(why)) = (self.floor, attest_fail.clone()) {
                return Err(TrustError::AttestationFailed {
                    what: art.asset.clone(),
                    why,
                });
            }
            let why = match self.floor {
                TrustLevel::Attested if !art.attests => {
                    format!("{} does not publish attestations", art.tool)
                }
                TrustLevel::Attested => "gh is not available to check the attestation".to_string(),
                _ => format!("{} is not in this installer's compiled-in pins", art.asset),
            };
            return Err(TrustError::FloorNotMet {
                floor: self.floor.as_str(),
                reached: level.as_str(),
                why,
            });
        }
        let mut admitted = seal(
            Verified {
                level,
                sha256: actual,
                independent: true,
                attested_by,
            },
            self,
        );
        // Below the attested floor a failed check does not stop the install,
        // but for a product that publishes attestations it is the one sign of
        // a swapped artifact: it is said, not dropped.
        if let Some(why) = attest_fail {
            admitted = admitted.note(format!(
                "{} failed its attestation check and is installed at `{}` only: {why}",
                art.asset,
                level.as_str()
            ));
        }
        Ok(admitted)
    }
}

fn verify_manifest_attestation(
    a: &dyn Attest,
    doc: &[u8],
    bundle: &[u8],
    scratch: Option<&Path>,
) -> Result<String, String> {
    let dir = super::attest::PrivateDir::new(scratch).map_err(|e| e.to_string())?;
    let d = dir.put("manifest.toml", doc).map_err(|e| e.to_string())?;
    let b = dir
        .put("manifest.attestation.jsonl", bundle)
        .map_err(|e| e.to_string())?;
    a.verify(&d, crate::location::INSTALLER_REPO, Some(&b))
}
