// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! Releases read through the maintainer's own `gh`: authenticated (no anonymous rate limit), and
//! the same view of a release the maintainer sees on GitHub.

use std::collections::BTreeMap;
use std::process::Command;

use crate::assemble::{AssetInfo, ReleaseInfo, Releases};

pub struct Gh;

/// `owner/name` of plain characters: what goes into an API path is never more than that.
fn plain_repo(repo: &str) -> bool {
    let mut parts = repo.split('/');
    let ok = |s: &str| {
        !s.is_empty()
            && !s.starts_with('.')
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
    };
    matches!((parts.next(), parts.next(), parts.next()), (Some(a), Some(b), None) if ok(a) && ok(b))
}

fn plain_tag(tag: &str) -> bool {
    !tag.is_empty()
        && tag
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.' || b == b'+')
}

fn gh(args: &[&str]) -> Result<Vec<u8>, String> {
    let out = Command::new("gh")
        .args(args)
        .output()
        .map_err(|e| format!("cannot run gh (is it installed and signed in?): {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "gh {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(out.stdout)
}

/// GitHub's `digest` field: `sha256:<hex>`.
fn digest_of(v: &serde_json::Value) -> Option<String> {
    let d = v.get("digest")?.as_str()?.strip_prefix("sha256:")?;
    (d.len() == 64 && d.bytes().all(|b| b.is_ascii_hexdigit())).then(|| d.to_ascii_lowercase())
}

impl Releases for Gh {
    fn release(&self, repo: &str, tag: &str) -> Result<ReleaseInfo, String> {
        if !plain_repo(repo) || !plain_tag(tag) {
            return Err(format!(
                "`{repo}` `{tag}` is not a plain repository and tag"
            ));
        }
        // the tags endpoint answers only for published releases: a draft is a 404 here
        let body = gh(&["api", &format!("repos/{repo}/releases/tags/{tag}")])
            .map_err(|e| format!("{repo} has no published release {tag}: {e}"))?;
        let v: serde_json::Value =
            serde_json::from_slice(&body).map_err(|e| format!("{repo} {tag}: {e}"))?;
        if v.get("draft").and_then(|d| d.as_bool()) == Some(true) {
            return Err(format!("{repo} {tag} is a draft"));
        }
        let published = v
            .get("published_at")
            .and_then(|p| p.as_str())
            .and_then(|p| p.get(..10))
            .ok_or_else(|| format!("{repo} {tag} has no publication date"))?
            .to_string();
        let mut assets = BTreeMap::new();
        for a in v
            .get("assets")
            .and_then(|a| a.as_array())
            .into_iter()
            .flatten()
        {
            let (Some(name), Some(id), Some(size)) = (
                a.get("name").and_then(|n| n.as_str()),
                a.get("id").and_then(|n| n.as_u64()),
                a.get("size").and_then(|n| n.as_u64()),
            ) else {
                return Err(format!("{repo} {tag}: an asset without a name, id or size"));
            };
            assets.insert(
                name.to_string(),
                AssetInfo {
                    id,
                    size,
                    digest: digest_of(a),
                },
            );
        }
        Ok(ReleaseInfo {
            published,
            prerelease: v.get("prerelease").and_then(|p| p.as_bool()) == Some(true),
            assets,
        })
    }

    fn small_asset(&self, repo: &str, asset: &AssetInfo, max: u64) -> Result<Vec<u8>, String> {
        if asset.size > max {
            return Err(format!(
                "asset {} is {} bytes, over {max}",
                asset.id, asset.size
            ));
        }
        let bytes = gh(&[
            "api",
            "-H",
            "Accept: application/octet-stream",
            &format!("repos/{repo}/releases/assets/{}", asset.id),
        ])?;
        if bytes.len() as u64 > max {
            return Err(format!("asset {} came back over {max} bytes", asset.id));
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_names_reach_the_api() {
        assert!(plain_repo("otmof-ops/cig-script"));
        for bad in ["a", "a/b/c", "../x", "a/..", "a/b?c", "a /b", "/b"] {
            assert!(!plain_repo(bad), "{bad}");
        }
        assert!(plain_tag("v1.20.0"));
        assert!(!plain_tag("v1/../x") && !plain_tag("") && !plain_tag("v1 2"));
    }

    #[test]
    fn github_digests_are_read_only_when_they_are_sha256() {
        let d = "4649fda09236ada8a2ade2460c33db9cfa4b2f4dba40186099118a6a3b2dc8e3";
        let v = serde_json::json!({ "digest": format!("sha256:{d}") });
        assert_eq!(digest_of(&v).as_deref(), Some(d));
        assert_eq!(digest_of(&serde_json::json!({ "digest": "md5:abc" })), None);
        assert_eq!(digest_of(&serde_json::json!({})), None);
    }
}
