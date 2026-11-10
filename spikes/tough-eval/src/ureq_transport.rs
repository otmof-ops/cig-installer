// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! A `tough::Transport` over `ureq`, the HTTP client the installer already links.
//!
//! `ureq` blocks, so each fetch runs on tokio's blocking pool. A body is read whole, up to
//! `max_bytes`, before it is handed to `tough`; that is right for metadata (a few kilobytes) and
//! wrong for release tarballs, which the installer fetches with its own resumable downloader and
//! checks against the length and SHA-256 that signed targets metadata carries.

use std::io::Read;
use tough::async_trait;
use tough::{Transport, TransportError, TransportErrorKind, TransportStream};
use url::Url;

#[derive(Debug, Clone)]
pub struct UreqTransport {
    agent: ureq::Agent,
    max_bytes: u64,
}

impl UreqTransport {
    pub fn new(max_bytes: u64) -> Self {
        Self {
            agent: ureq::AgentBuilder::new()
                .timeout(std::time::Duration::from_secs(30))
                .build(),
            max_bytes,
        }
    }
}

#[async_trait]
impl Transport for UreqTransport {
    async fn fetch(&self, url: Url) -> Result<TransportStream, TransportError> {
        if !matches!(url.scheme(), "http" | "https") {
            return Err(TransportError::new(
                TransportErrorKind::UnsupportedUrlScheme,
                url,
            ));
        }
        let agent = self.agent.clone();
        let max = self.max_bytes;
        let target = url.to_string();
        let body = tokio::task::spawn_blocking(move || {
            let resp = agent.get(&target).call().map_err(|e| match e {
                // TUF 5.2.2, "if version N+1 of root is not available, stop": a 404 is the
                // one status that means "does not exist", so it is the only one mapped to it.
                ureq::Error::Status(404, _) => {
                    TransportError::new(TransportErrorKind::FileNotFound, &target)
                }
                other => TransportError::new_with_cause(TransportErrorKind::Other, &target, other),
            })?;
            let mut buf = Vec::new();
            resp.into_reader()
                .take(max.saturating_add(1))
                .read_to_end(&mut buf)
                .map_err(|e| {
                    TransportError::new_with_cause(TransportErrorKind::Other, &target, e)
                })?;
            if buf.len() as u64 > max {
                return Err(TransportError::new(TransportErrorKind::Other, &target));
            }
            Ok(buf)
        })
        .await
        .map_err(|e| {
            TransportError::new_with_cause(TransportErrorKind::Other, url.as_str(), e)
        })??;
        Ok(Box::pin(futures::stream::once(async move {
            Ok(tough::Bytes::from(body))
        })))
    }
}
