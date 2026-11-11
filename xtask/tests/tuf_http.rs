// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! The default transport: `TufTrust` over the installer's own HTTP client, against a local server.
//! A 404 ends the root chain (and only a 404 does); any other status is a refusal.

use cigi_source::{SeenState, Trust, TufConfig, TufTrust};
use jiff::Timestamp;
use std::path::Path;
use xtask::tuf::{self, KeyFile};

fn key(dir: &Path, n: &str) -> KeyFile {
    let p = dir.join(n);
    tuf::keygen(&p, false).expect("keygen");
    KeyFile::read(&p).expect("read")
}

/// Serves `dir` until dropped; `broken` makes every request answer 500.
struct Server {
    srv: std::sync::Arc<tiny_http::Server>,
    url: String,
    th: Option<std::thread::JoinHandle<()>>,
}
impl Server {
    fn start(dir: std::path::PathBuf, status: Option<u16>) -> Server {
        let srv = std::sync::Arc::new(tiny_http::Server::http("127.0.0.1:0").expect("bind"));
        let url = format!("http://{}/", srv.server_addr());
        let s2 = srv.clone();
        let th = std::thread::spawn(move || {
            for rq in s2.incoming_requests() {
                let name = rq.url().trim_start_matches('/').to_string();
                let path = dir.join(&name);
                let r = match (status, std::fs::read(&path)) {
                    (Some(code), _) => {
                        tiny_http::Response::from_data(Vec::new()).with_status_code(code)
                    }
                    (None, Ok(b)) if !name.contains("..") => tiny_http::Response::from_data(b),
                    _ => tiny_http::Response::from_data(Vec::new()).with_status_code(404),
                };
                let _ = rq.respond(r);
            }
        });
        Server {
            srv,
            url,
            th: Some(th),
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.srv.unblock();
        if let Some(t) = self.th.take() {
            let _ = t.join();
        }
    }
}

fn repo() -> (tempfile::TempDir, std::path::PathBuf, String) {
    let tmp = tempfile::tempdir().expect("tmp");
    let k = |n: &str| key(tmp.path(), n);
    let meta = tmp.path().join("meta");
    let now = Timestamp::now();
    let (rel, snap, ts) = (k("rel"), k("snap"), k("ts"));
    tuf::init(&tuf::Init {
        dir: &meta,
        root_keys: vec![k("ra"), k("rb"), k("rc")],
        root_threshold: 2,
        targets_key: k("tg"),
        releases_key: rel.clone(),
        snapshot_key: snap.clone(),
        timestamp_key: ts.clone(),
        now,
    })
    .expect("init");
    let mut fx = cigi_testkit::Fixture::new();
    fx.product("cig", "1.0.0");
    fx.carton("0.1.0", &[("cig", "1.0.0")]);
    fx.publish("stable");
    let text = std::fs::read_to_string(fx.releases.join("manifest-stable.toml")).expect("m");
    let es = tuf::entries_for(&text).expect("entries");
    tuf::sign_releases(&meta, &rel, &es, now, 30).expect("releases");
    tuf::sign_snapshot(&meta, &snap, now, 30).expect("snapshot");
    tuf::sign_timestamp(&meta, &ts, now, 7).expect("timestamp");
    (tmp, meta, text)
}

fn client(root: Vec<u8>, url: &str, ds: &Path) -> TufTrust {
    TufTrust::new(TufConfig {
        root,
        metadata_url: url::Url::parse(url).expect("url"),
        datastore: ds.to_path_buf(),
        min_snapshot_version: 1,
        min_targets_version: 1,
    })
    .with_attest(std::sync::Arc::new(cigi_source::NoAttest))
    .with_pins(&[])
}

fn seen() -> SeenState {
    SeenState {
        channel: Some("stable".into()),
        ..SeenState::default()
    }
}

#[test]
fn over_http_a_404_ends_the_root_chain_and_the_manifest_is_admitted() {
    let (tmp, meta, text) = repo();
    let root = std::fs::read(meta.join("1.root.json")).expect("root");
    let srv = Server::start(meta, None);
    let t = client(root, &srv.url, &tmp.path().join("ds"));
    t.admit_manifest(text.as_bytes(), None, None, &seen())
        .expect("admitted over http");
}

#[test]
fn over_http_a_server_error_is_a_refusal_not_the_end_of_the_chain() {
    let (tmp, meta, text) = repo();
    let root = std::fs::read(meta.join("1.root.json")).expect("root");
    let srv = Server::start(meta, Some(500));
    let t = client(root, &srv.url, &tmp.path().join("ds"));
    assert!(t
        .admit_manifest(text.as_bytes(), None, None, &seen())
        .is_err());
}

#[test]
fn a_non_http_url_is_refused_by_the_default_transport() {
    let (tmp, meta, text) = repo();
    let root = std::fs::read(meta.join("1.root.json")).expect("root");
    let t = client(
        root,
        url::Url::from_directory_path(&meta).expect("url").as_str(),
        &tmp.path().join("ds"),
    );
    assert!(t
        .admit_manifest(text.as_bytes(), None, None, &seen())
        .is_err());
}
