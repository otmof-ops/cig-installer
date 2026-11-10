// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! The seam: `tough` carried by the installer's own HTTP client (`ureq`) against a local server,
//! with no `reqwest` and no second TLS stack. A 404 for the next root must end the root chain
//! (that is what `FileNotFound` is for), and a tampered target must still be refused.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use tough::{IntoVec, RepositoryLoader, TargetName};
use tough_eval::fixture::*;
use tough_eval::ureq_transport::UreqTransport;

/// Serves `root` read-only. Anything outside it, and anything missing, is a 404.
fn serve(root: PathBuf) -> (String, Arc<tiny_http::Server>) {
    let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").expect("bind"));
    let base = format!("http://{}", server.server_addr().to_ip().expect("ip"));
    let s = server.clone();
    std::thread::spawn(move || {
        for req in s.incoming_requests() {
            let rel = req.url().trim_start_matches('/').to_string();
            let ok = !rel.split('/').any(|p| p == ".." || p.is_empty());
            let file = root.join(&rel);
            let reply = match (ok, std::fs::read(&file)) {
                (true, Ok(bytes)) => req.respond(tiny_http::Response::from_data(bytes)),
                _ => req.respond(tiny_http::Response::empty(404)),
            };
            let _ = reply;
        }
    });
    (base, server)
}

async fn client(root: &[u8], base: &str, ds: &Path) -> tough::error::Result<tough::Repository> {
    std::fs::create_dir_all(ds).unwrap();
    let url = |p: &str| url::Url::parse(&format!("{base}/{p}/")).unwrap();
    RepositoryLoader::new(&root, url("metadata"), url("targets"))
        .transport(UreqTransport::new(1 << 20))
        .datastore(ds)
        .load()
        .await
}

#[tokio::test]
async fn tough_over_ureq_loads_and_reads_a_target() {
    let w = World::new().await;
    let m = w
        .publish(
            "m",
            &[File::new("cig-1.20.1-linux-x86_64", b"new build")],
            &Release::at(1),
        )
        .await;
    let (base, _s) = serve(m.meta.parent().unwrap().to_path_buf());
    let repo = client(&w.trusted_root(), &base, &w.path("ds"))
        .await
        .unwrap();
    let stream = repo
        .read_target(&TargetName::new("cig-1.20.1-linux-x86_64").unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stream.into_vec().await.unwrap(), b"new build");
}

#[tokio::test]
async fn tough_over_ureq_refuses_a_tampered_target() {
    let w = World::new().await;
    let m = w
        .publish(
            "m",
            &[File::new("cig-1.20.1-linux-x86_64", b"new build")],
            &Release::at(1),
        )
        .await;
    let f = std::fs::read_dir(&m.targets)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::write(&f, b"evil build").unwrap();
    let (base, _s) = serve(m.meta.parent().unwrap().to_path_buf());
    let repo = client(&w.trusted_root(), &base, &w.path("ds"))
        .await
        .unwrap();
    let stream = repo
        .read_target(&TargetName::new("cig-1.20.1-linux-x86_64").unwrap())
        .await
        .unwrap()
        .unwrap();
    assert!(stream.into_vec().await.is_err());
}

#[tokio::test]
async fn a_server_error_is_not_a_missing_root() {
    // A mirror that answers 500 to everything must fail the load, not end the root chain quietly.
    let w = World::new().await;
    let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
    let base = format!("http://{}", server.server_addr().to_ip().unwrap());
    let s = server.clone();
    std::thread::spawn(move || {
        for req in s.incoming_requests() {
            let _ = req.respond(tiny_http::Response::empty(500));
        }
    });
    assert!(client(&w.trusted_root(), &base, &w.path("ds"))
        .await
        .is_err());
}
