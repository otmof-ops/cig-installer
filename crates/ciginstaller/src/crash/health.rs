// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! Each route's last success and last failure (#28 section 4, #13), kept in
//! `<home>/ashtray/.routes.json`: the reason, never a secret, never a report's content.
//! `doctor` and `crash status` read it. Writing it is best effort: a report is never lost
//! because a status line could not be saved.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::ashtray_dir;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RouteLog {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_success: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_failure: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    routes: BTreeMap<String, RouteLog>,
}

pub fn path(root: &Path) -> PathBuf {
    ashtray_dir(root).join(".routes.json")
}

pub fn load(root: &Path) -> BTreeMap<String, RouteLog> {
    std::fs::read_to_string(path(root))
        .ok()
        .and_then(|t| serde_json::from_str::<File>(&t).ok())
        .map(|f| f.routes)
        .unwrap_or_default()
}

/// Note that `route` worked (`Ok`) or did not (`Err(reason)`).
pub fn record(root: &Path, route: &str, outcome: Result<(), &str>) {
    let mut routes = load(root);
    let e = routes.entry(route.to_string()).or_default();
    let now = cigi_home::now_rfc3339();
    match outcome {
        Ok(()) => e.last_success = Some(now),
        Err(why) => {
            e.last_failure = Some(now);
            e.last_error = Some(super::truncate(why, 300));
        }
    }
    let dir = ashtray_dir(root);
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let Ok(text) = serde_json::to_string_pretty(&File { routes }) else {
        return;
    };
    let tmp = dir.join(format!(".routes.json.{}.tmp", std::process::id()));
    if std::fs::write(&tmp, text).is_ok() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        }
        let _ = std::fs::rename(&tmp, path(root));
    }
}

/// An epoch second as RFC 3339 UTC (days-to-civil, std only).
pub fn rfc3339_of(epoch: i64) -> String {
    let days = epoch.div_euclid(86_400);
    let rem = epoch.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn epoch_to_rfc3339() {
        assert_eq!(super::rfc3339_of(0), "1970-01-01T00:00:00Z");
        assert_eq!(super::rfc3339_of(1_794_271_271), "2026-11-10T00:41:11Z");
    }
}
