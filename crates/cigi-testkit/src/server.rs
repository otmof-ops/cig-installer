// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! The fixture server: `tiny_http` on `127.0.0.1:0` over a `releases/` directory, recording every
//! request. Rules per path make it misbehave on purpose (redirect, stalled body, a status); a body that ends early needs `ShortBodyServer`.

use crate::must;
use crate::recorder::{check_no_telemetry, Recorded};
use std::collections::{BTreeSet, HashMap};
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;
use tiny_http::{Header, Response, Server, StatusCode};

/// How the server treats one path instead of serving the file.
#[derive(Debug, Clone)]
pub enum Rule {
    /// `302` to this absolute URL (another host, or the same one).
    Redirect(String),
    /// Send the headers and the first byte, then wait this long before the rest.
    Stall(Duration),
    /// Answer this status with an empty body.
    Status(u16),
}

type Rules = Arc<Mutex<HashMap<String, Rule>>>;
type Log = Arc<Mutex<Vec<Recorded>>>;

pub struct FixtureServer {
    server: Arc<Server>,
    addr: std::net::SocketAddr,
    log: Log,
    rules: Rules,
    root: PathBuf,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

struct Stalled {
    data: Cursor<Vec<u8>>,
    delay: Duration,
    first: bool,
}
impl Read for Stalled {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.first {
            self.first = false;
            let n = buf.len().min(1);
            return self.data.read(&mut buf[..n]);
        }
        std::thread::sleep(self.delay);
        self.data.read(buf)
    }
}

fn header(k: &str, v: &str) -> Option<Header> {
    Header::from_bytes(k.as_bytes(), v.as_bytes()).ok()
}

impl FixtureServer {
    pub(crate) fn start(root: &Path) -> FixtureServer {
        let server = Arc::new(must(
            Server::http("127.0.0.1:0").map_err(|e| e.to_string()),
            "bind loopback",
        ));
        let addr = must(
            server.server_addr().to_ip().ok_or("not an ip address"),
            "server address",
        );
        let log: Log = Arc::default();
        let rules: Rules = Arc::default();
        let stop = Arc::new(AtomicBool::new(false));
        let handle = {
            let (server, log, rules, stop, root) = (
                server.clone(),
                log.clone(),
                rules.clone(),
                stop.clone(),
                root.to_path_buf(),
            );
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    match server.recv_timeout(Duration::from_millis(50)) {
                        Ok(Some(req)) => {
                            let (log, rules, root) = (log.clone(), rules.clone(), root.clone());
                            std::thread::spawn(move || handle(req, &log, &rules, &root));
                        }
                        Ok(None) => {}
                        Err(_) => break,
                    }
                }
            })
        };
        FixtureServer {
            server,
            addr,
            log,
            rules,
            root: root.to_path_buf(),
            stop,
            handle: Some(handle),
        }
    }

    /// `http://127.0.0.1:PORT`: usable as a mirror (loopback http is allowed).
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }
    pub fn port(&self) -> u16 {
        self.addr.port()
    }
    /// Every request so far, in arrival order.
    pub fn requests(&self) -> Vec<Recorded> {
        self.lock_log().clone()
    }
    pub fn paths(&self) -> Vec<String> {
        self.requests().into_iter().map(|r| r.path).collect()
    }
    pub fn clear(&self) {
        self.lock_log().clear();
    }
    /// Misbehave on `path` (e.g. `/cig-v1.0.0-linux-x86_64`).
    pub fn rule(&self, path: &str, rule: Rule) {
        match self.rules.lock() {
            Ok(mut r) => {
                r.insert(path.to_string(), rule);
            }
            Err(p) => {
                p.into_inner().insert(path.to_string(), rule);
            }
        }
    }
    /// The #19 rules over everything recorded so far.
    pub fn assert_no_telemetry(&self) -> Result<(), Vec<String>> {
        let known: BTreeSet<String> = std::fs::read_dir(&self.root)
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        check_no_telemetry(&self.requests(), &known)
    }

    fn lock_log(&self) -> std::sync::MutexGuard<'_, Vec<Recorded>> {
        match self.log.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        }
    }
}

impl Drop for FixtureServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.server.unblock();
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn handle(req: tiny_http::Request, log: &Log, rules: &Rules, root: &Path) {
    let full = req.url().to_string();
    let (path, query) = match full.split_once('?') {
        Some((p, q)) => (p.to_string(), Some(q.to_string())),
        None => (full.clone(), None),
    };
    let rec = Recorded {
        method: req.method().as_str().to_string(),
        path: path.clone(),
        query,
        headers: req
            .headers()
            .iter()
            .map(|h| {
                (
                    h.field.as_str().as_str().to_string(),
                    h.value.as_str().to_string(),
                )
            })
            .collect(),
    };
    match log.lock() {
        Ok(mut l) => l.push(rec.clone()),
        Err(p) => p.into_inner().push(rec.clone()),
    }
    let rule = rules.lock().ok().and_then(|r| r.get(&path).cloned());
    let status = |c: u16| Response::from_data(Vec::new()).with_status_code(StatusCode(c));
    if rec.method != "GET" {
        let _ = req.respond(status(405));
        return;
    }
    if let Some(Rule::Redirect(to)) = &rule {
        let mut r = status(302);
        if let Some(h) = header("Location", to) {
            r = r.with_header(h);
        }
        let _ = req.respond(r);
        return;
    }
    if let Some(Rule::Status(c)) = &rule {
        let _ = req.respond(status(*c));
        return;
    }
    let name = path.strip_prefix('/').unwrap_or(&path);
    let plain =
        !name.is_empty() && !name.contains('/') && !name.contains("..") && !name.contains('\\');
    let data = if plain {
        std::fs::read(root.join(name)).ok()
    } else {
        None
    };
    let Some(data) = data else {
        let _ = req.respond(status(404));
        return;
    };
    match rule {
        Some(Rule::Stall(d)) => {
            let n = data.len();
            let mut r = Response::new(
                StatusCode(200),
                Vec::new(),
                Stalled {
                    data: Cursor::new(data),
                    delay: d,
                    first: true,
                },
                Some(n),
                None,
            );
            if let Some(h) = header("Connection", "close") {
                r = r.with_header(h);
            }
            let _ = req.respond(r);
        }
        _ => {
            let _ = req.respond(Response::from_data(data));
        }
    }
}

/// A one-purpose server for the transfer that ends early: every request is answered with headers
/// that claim `claimed` bytes, `body` is sent, and the connection is closed. (`tiny_http` keeps
/// connections open, so it cannot do this; this is plain `std::net`.)
pub struct ShortBodyServer {
    addr: std::net::SocketAddr,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl ShortBodyServer {
    pub fn start(body: Vec<u8>, claimed: usize) -> ShortBodyServer {
        let listener = must(std::net::TcpListener::bind("127.0.0.1:0"), "bind loopback");
        let addr = must(listener.local_addr(), "local addr");
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let handle = std::thread::spawn(move || {
            for conn in listener.incoming() {
                if flag.load(Ordering::Relaxed) {
                    break;
                }
                let Ok(mut c) = conn else { continue };
                let _ = c.set_read_timeout(Some(Duration::from_secs(2)));
                let mut seen = Vec::new();
                let mut buf = [0u8; 512];
                while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
                    match c.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => seen.extend_from_slice(&buf[..n]),
                    }
                }
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {claimed}\r\nConnection: close\r\n\r\n"
                );
                let _ = c.write_all(head.as_bytes());
                let _ = c.write_all(&body);
                let _ = c.flush();
            }
        });
        ShortBodyServer {
            addr,
            stop,
            handle: Some(handle),
        }
    }
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }
}

impl Drop for ShortBodyServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = std::net::TcpStream::connect(self.addr);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}
