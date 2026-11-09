// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0
//! `channel`, `channel <name>`: which manifest this machine follows (SPEC 10: "switch channel;
//! refetch manifest; no install"). The new channel's manifest is fetched and admitted before the
//! setting changes, so a channel with no manifest, or one that fails its checks, is never set.

use cigi_home::Config;
use cigi_source::FetchError;
use cigi_ui::{InstallerError, Report, EXIT_REFUSED};
use serde_json::json;

use super::install::no_such_channel;
use super::Outcome;
use crate::cx::Cx;
use crate::errors::ie;
use crate::flows::settle;

pub fn run(cx: &mut Cx, name: Option<&str>) -> Result<Outcome, InstallerError> {
    let current = cx.channel();
    let Some(name) = name else {
        return Ok(
            Report::done(format!("this machine follows the {current} channel"))
                .data(json!({ "channel": current }))
                .into(),
        );
    };
    if Config::validate("channel", name).is_err() {
        return Err(no_such_channel(name));
    }
    if cx.cfg.get_file("channel").as_deref() == Some(name) && current == name {
        return Ok(
            Report::done(format!("this machine already follows the {name} channel"))
                .data(json!({ "channel": name, "changed": false }))
                .into(),
        );
    }
    // the layout (and its modes) and the lock first: the fetch writes the manifest cache
    cx.lock_and_recover()?;
    let admitted = cx.fetch_manifest(name).map_err(|e| match e {
        FetchError::NotFound { .. } => InstallerError::new(
            "E1812",
            format!("the {name} channel has no manifest yet, so this machine stays on {current}"),
        )
        .with_fix(format!("ciginstaller channel {current}"))
        .with_exit(EXIT_REFUSED),
        other => ie(other),
    })?;
    // the checks `manifest()` makes of every admitted document: a replay of another document at
    // a sequence already seen is refused, and the trust notes are said
    cx.check_digest_record(name, &admitted)?;
    for n in admitted.notes() {
        cx.note(n);
    }
    let newest = admitted.latest(false).map(|c| c.version.to_string());
    let mut state = cx.state.clone();
    cx.record_manifest_digest(&mut state, name, admitted.sequence);
    let mut txn = cx.begin(&format!("channel {name}"))?;
    let set = (|| {
        cx.cfg.set_in(&mut txn, "channel", name).map_err(ie)?;
        state.save_in(&mut txn).map_err(ie)
    })();
    settle(txn, set)?;
    cx.state = state;
    let mut r = Report::done(format!("this machine now follows the {name} channel"));
    if let Some(v) = &newest {
        r = r
            .detail(format!(
                "the newest carton there is {v}; nothing was installed"
            ))
            .next("ciginstaller install carton");
    }
    if std::env::var_os("CIGI_CHANNEL").is_some() {
        r = r.detail("CIGI_CHANNEL is set in this shell, and it outranks the setting");
    }
    Ok(
        r.data(json!({ "channel": name, "changed": true, "newest": newest }))
            .into(),
    )
}
