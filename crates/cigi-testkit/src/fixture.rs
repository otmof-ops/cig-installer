// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0
//! The fixture release source (section 14.1): scratch homes, fake products, a mirror directory,
//! `publish`, and the means to run the built binaries against all of it.

use crate::products::{host_target, ProductBuilder};
use crate::server::FixtureServer;
use crate::{must, sha256_hex};
use cigi_home::Home;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;
use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime};

/// What `publish` wrote.
#[derive(Debug, Clone)]
pub struct ManifestPaths {
    pub manifest: PathBuf,
    /// `None` when `PublishOpts::no_sidecar` was set.
    pub sidecar: Option<PathBuf>,
    pub sequence: u64,
    /// Every asset and sidecar file now in `releases/` for the declared products.
    pub assets: Vec<PathBuf>,
}

/// Knobs for one `publish`.
#[derive(Debug, Clone, Default)]
pub struct PublishOpts {
    /// Use this sequence instead of previous + 1 (to publish a manifest that goes backwards: E1824).
    pub sequence: Option<u64>,
    /// `expires` this many days from now (negative: already expired, E1823). Default 30.
    pub expires_in_days: Option<i64>,
    /// The manifest's `.sha256` names a different digest (E1827).
    pub bad_sidecar: bool,
    /// No manifest `.sha256` at all (E1827).
    pub no_sidecar: bool,
}

struct CartonSpec {
    version: String,
    products: Vec<(String, String)>,
    notes: String,
    yanked: Option<String>,
}

pub struct Fixture {
    pub tmp: TempDir,
    /// `$CIGSCRIPT_HOME` for child processes: `<tmp>/home/.cigscript`.
    pub home: Home,
    /// `$HOME` for child processes: `<tmp>/home`.
    pub user_home: PathBuf,
    pub project: PathBuf,
    /// A flat mirror directory: `manifest-<channel>.toml`, assets and sidecars by file name.
    pub releases: PathBuf,
    /// A second shim name on cigo's tool definition (`cigo` is an archive, which may have several):
    /// for proving what the installer does with a name the shim binary does not serve.
    pub extra_cigo_shim: Option<String>,
    products: Vec<ProductBuilder>,
    cartons: Vec<CartonSpec>,
    sequences: BTreeMap<String, u64>,
}

impl Default for Fixture {
    fn default() -> Self {
        Self::new()
    }
}

impl Fixture {
    /// Scratch HOME, scratch `CIGSCRIPT_HOME` (not created: the installer makes it), an empty project
    /// directory and an empty `releases/`. Sets nothing in this process's environment.
    pub fn new() -> Fixture {
        let tmp = must(
            tempfile::Builder::new().prefix("cigi-fixture-").tempdir(),
            "scratch dir",
        );
        let user_home = tmp.path().join("home");
        let project = tmp.path().join("project");
        let releases = tmp.path().join("releases");
        for d in [&user_home, &project, &releases] {
            must(std::fs::create_dir_all(d), "create fixture dir");
        }
        let home = Home::at(user_home.join(".cigscript"));
        Fixture {
            tmp,
            home,
            user_home,
            project,
            releases,
            extra_cigo_shim: None,
            products: Vec::new(),
            cartons: Vec::new(),
            sequences: BTreeMap::new(),
        }
    }

    /// `$CIGSCRIPT_HOME` as a path.
    pub fn cigscript(&self) -> PathBuf {
        self.user_home.join(".cigscript")
    }

    /// A fake product version (replaces one declared before with the same tool and version).
    pub fn product(&mut self, tool: &str, version: &str) -> &mut ProductBuilder {
        if let Some(i) = self
            .products
            .iter()
            .position(|p| p.tool == tool && p.version == version)
        {
            self.products[i] = ProductBuilder::new(tool, version);
            return &mut self.products[i];
        }
        self.products.push(ProductBuilder::new(tool, version));
        let n = self.products.len();
        &mut self.products[n - 1]
    }

    /// A carton of exactly these product versions. A product not declared yet is declared plain.
    pub fn carton(&mut self, v: &str, products: &[(&str, &str)]) -> &mut Self {
        for (t, pv) in products {
            if !self
                .products
                .iter()
                .any(|p| p.tool == *t && p.version == *pv)
            {
                self.products.push(ProductBuilder::new(t, pv));
            }
        }
        let spec = CartonSpec {
            version: v.into(),
            products: products
                .iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect(),
            notes: format!("fixture carton {v}"),
            yanked: None,
        };
        match self.cartons.iter().position(|c| c.version == v) {
            Some(i) => self.cartons[i] = spec,
            None => self.cartons.push(spec),
        }
        self
    }

    /// Mark a declared carton yanked (`"carton <v>" = reason`).
    pub fn yank_carton(&mut self, v: &str, reason: &str) -> &mut Self {
        if let Some(c) = self.cartons.iter_mut().find(|c| c.version == v) {
            c.yanked = Some(reason.into());
        }
        self
    }

    /// Write `manifest-<channel>.toml` (+ `.sha256`, sequence = previous + 1) and every declared
    /// product's asset (+ sidecar) into `releases/`.
    pub fn publish(&mut self, channel: &str) -> ManifestPaths {
        self.publish_with(channel, &PublishOpts::default())
    }

    pub fn publish_with(&mut self, channel: &str, opts: &PublishOpts) -> ManifestPaths {
        let mut assets = Vec::new();
        let mut builds = Vec::new();
        for p in &self.products {
            let b = p.build();
            let path = self.releases.join(&b.name);
            must(std::fs::write(&path, &b.served), "write asset");
            assets.push(path);
            let side = self.releases.join(format!("{}.sha256", b.name));
            match &b.sidecar {
                Some(text) => {
                    must(std::fs::write(&side, text), "write sidecar");
                    assets.push(side);
                }
                None => {
                    let _ = std::fs::remove_file(&side);
                }
            }
            builds.push((p.clone(), b));
        }
        let seq = opts
            .sequence
            .unwrap_or_else(|| self.sequences.get(channel).copied().unwrap_or(0) + 1);
        self.sequences.insert(channel.to_string(), seq);
        let now = OffsetDateTime::now_utc()
            .replace_nanosecond(0)
            .unwrap_or_else(|_| OffsetDateTime::now_utc());
        let days = opts.expires_in_days.unwrap_or(30);
        let expires = now + Duration::days(days);
        // An expired manifest was generated before it expired (a manifest
        // whose `expires` comes before `generated` is malformed, E1825).
        let generated = if expires <= now {
            expires - Duration::days(7)
        } else {
            now
        };
        let text = self.render_manifest(channel, seq, generated, expires, &builds);
        let manifest = self.releases.join(format!("manifest-{channel}.toml"));
        must(std::fs::write(&manifest, &text), "write manifest");
        let side_path = self
            .releases
            .join(format!("manifest-{channel}.toml.sha256"));
        let sidecar = if opts.no_sidecar {
            let _ = std::fs::remove_file(&side_path);
            None
        } else {
            let digest = if opts.bad_sidecar {
                sha256_hex(b"not this manifest")
            } else {
                sha256_hex(text.as_bytes())
            };
            must(
                std::fs::write(&side_path, format!("{digest}  manifest-{channel}.toml\n")),
                "write manifest sidecar",
            );
            Some(side_path)
        };
        ManifestPaths {
            manifest,
            sidecar,
            sequence: seq,
            assets,
        }
    }

    fn render_manifest(
        &self,
        channel: &str,
        seq: u64,
        generated: OffsetDateTime,
        expires: OffsetDateTime,
        builds: &[(ProductBuilder, crate::products::Built)],
    ) -> String {
        let fmt = |t: OffsetDateTime| must(t.format(&Rfc3339), "format time");
        let host = host_target();
        let mut o = String::new();
        o.push_str(&format!(
            "format    = 1\nchannel   = \"{channel}\"\nsequence  = {seq}\ngenerated = {}\nexpires   = {}\n\n",
            fmt(generated),
            fmt(expires)
        ));
        // tool definitions (section 6.1), for the tools that appear
        let mut tools: Vec<&str> = builds.iter().map(|(p, _)| p.tool.as_str()).collect();
        tools.sort_unstable();
        tools.dedup();
        for t in &tools {
            let mut def = tool_def(t, &host);
            if let (true, Some(extra)) = (*t == "cigo", &self.extra_cigo_shim) {
                def = def.replace(
                    "windows_path = \"bin/cigo.cmd\" } ]",
                    &format!(
                        "windows_path = \"bin/cigo.cmd\" }}, {{ name = \"{extra}\", path = \"bin/cigo\" }} ]"
                    ),
                );
            }
            o.push_str(&def);
        }
        // builds
        let mut by_tool: BTreeMap<&str, Vec<&(ProductBuilder, crate::products::Built)>> =
            BTreeMap::new();
        for b in builds {
            by_tool.entry(b.0.tool.as_str()).or_default().push(b);
        }
        for (tool, list) in &by_tool {
            for (p, b) in list {
                let key = if p.tool == "cigo" {
                    "any".to_string()
                } else {
                    host.clone()
                };
                o.push_str(&format!(
                    "[build.{tool}.\"{v}\"]\nreleased = 2026-11-02\n[build.{tool}.\"{v}\".{key}]\nsha256 = \"{}\"\nsize   = {}\n\n",
                    b.sha256,
                    b.size,
                    v = p.version
                ));
            }
        }
        // cartons
        for c in &self.cartons {
            o.push_str(&format!(
                "[carton.\"{}\"]\nreleased = 2026-11-12\nnotes = \"{}\"\ntested = []\n",
                c.version, c.notes
            ));
            for (t, v) in &c.products {
                o.push_str(&format!("{t} = \"{v}\"\n"));
            }
            let has = |n: &str| c.products.iter().any(|(t, _)| t == n);
            let pick = |names: &[&str]| -> Vec<String> {
                names
                    .iter()
                    .filter(|n| has(n))
                    .map(|n| format!("\"{n}\""))
                    .collect()
            };
            let (dev, builder) = (pick(&["cig", "cigo"]), pick(&["cig", "cigo", "cigcompile"]));
            o.push_str(&format!(
                "[carton.\"{}\".packs]\ndev = [ {} ]\nbuilder = [ {} ]\n\n",
                c.version,
                dev.join(", "),
                builder.join(", ")
            ));
        }
        // product requirements
        for (p, _) in builds {
            if let Some(r) = &p.requires {
                o.push_str(&format!(
                    "[product.{}.\"{}\"]\nrequires = {{ {r} }}\n\n",
                    p.tool, p.version
                ));
            }
        }
        // yanks
        let mut yanks: Vec<String> = Vec::new();
        for (p, _) in builds {
            if let Some(r) = &p.yanked {
                yanks.push(format!(
                    "\"{} {}\" = \"{}\"",
                    p.tool,
                    p.version,
                    r.replace('"', "'")
                ));
            }
        }
        for c in &self.cartons {
            if let Some(r) = &c.yanked {
                yanks.push(format!(
                    "\"carton {}\" = \"{}\"",
                    c.version,
                    r.replace('"', "'")
                ));
            }
        }
        if !yanks.is_empty() {
            o.push_str("[yanked]\n");
            for y in yanks {
                o.push_str(&y);
                o.push('\n');
            }
        }
        o
    }

    /// Start a loopback server over `releases/`; every request is recorded.
    pub fn serve(&self) -> FixtureServer {
        FixtureServer::start(&self.releases)
    }

    /// The installer binary with scratch HOME and CIGSCRIPT_HOME, the mirror set to `file://<releases>`,
    /// `NO_COLOR=1`, `TERM=dumb`, and `PATH` = scratch `bin` first. Run from the project directory.
    pub fn cigi(&self) -> Command {
        self.cigi_with_mirror(&self.mirror_file_url())
    }

    /// As `cigi`, with `CIGI_MIRROR` set to `mirror` (for example a `FixtureServer::url()`).
    pub fn cigi_with_mirror(&self, mirror: &str) -> Command {
        let mut c = Command::new(binary_path("ciginstaller"));
        self.child_env(&mut c);
        c.env("CIGI_MIRROR", mirror).current_dir(&self.project);
        c
    }

    pub fn mirror_file_url(&self) -> String {
        format!("file://{}", self.releases.display())
    }

    /// Run `<CIGSCRIPT_HOME>/bin/<name>` (a shim the installer made) in `cwd`.
    pub fn run_shim(&self, name: &str, cwd: &Path, args: &[&str]) -> Output {
        let mut c = Command::new(self.cigscript().join("bin").join(name));
        self.child_env(&mut c);
        c.current_dir(cwd).args(args);
        must(output_retrying(&mut c), &format!("run shim {name}"))
    }

    fn child_env(&self, c: &mut Command) {
        let bin = self.cigscript().join("bin");
        let path =
            match std::env::join_paths([bin.as_path(), Path::new("/usr/bin"), Path::new("/bin")]) {
                Ok(p) => p,
                Err(_) => bin.into_os_string(),
            };
        c.env_clear()
            .env("HOME", &self.user_home)
            .env("CIGSCRIPT_HOME", self.cigscript())
            .env("NO_COLOR", "1")
            .env("TERM", "dumb")
            .env("LANG", "C")
            .env("PATH", path);
    }
}

/// Where a workspace binary was built: `CIGI_BIN` (for `ciginstaller`), `CARGO_BIN_EXE_<name>` at
/// run time, else next to the running test's `deps/` directory.
pub fn binary_path(name: &str) -> PathBuf {
    if name == "ciginstaller" {
        if let Some(p) = std::env::var_os("CIGI_BIN") {
            return PathBuf::from(p);
        }
    }
    if let Some(p) = std::env::var_os(format!("CARGO_BIN_EXE_{name}")) {
        return PathBuf::from(p);
    }
    let exe = std::env::current_exe().unwrap_or_default();
    let dir = exe.parent().and_then(|d| {
        if d.ends_with("deps") {
            d.parent()
        } else {
            Some(d)
        }
    });
    dir.unwrap_or_else(|| Path::new("."))
        .join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
}

fn tool_def(tool: &str, host: &str) -> String {
    let binary = |name: &str, repo: &str| {
        format!(
            "[tool.{tool}]\nrepo    = \"{repo}\"\ntag     = \"v{{version}}\"\nlayout  = \"binary\"\nasset   = \"{name}-v{{version}}-{{os}}-{{arch}}{{exe}}\"\nsidecar = \"{{asset}}.sha256\"\nshims   = [ {{ name = \"{name}\", path = \"bin/{name}\", windows_path = \"bin/{name}.exe\" }} ]\ntargets = [ \"{host}\" ]\nattests = false\n\n"
        )
    };
    match tool {
        "cig" => binary("cig", "otmof-ops/cig-script"),
        "cigcompile" => binary("cigcompile", "otmof-ops/cig-compile"),
        "cigo" => "[tool.cigo]\nrepo    = \"otmof-ops/cig-orchestrator\"\ntag     = \"v{version}\"\nlayout  = \"archive\"\nasset   = \"cig-orchestrator-v{version}.tar.gz\"\nsidecar = \"{asset}.sha256\"\nstrip   = 1\nshims   = [ { name = \"cigo\", path = \"bin/cigo\", windows_path = \"bin/cigo.cmd\" } ]\ntargets = [ \"any\" ]\nattests = false\n\n".to_string(),
        "ciginstaller" => format!(
            "[tool.ciginstaller]\nrepo    = \"otmof-ops/cig-installer\"\ntag     = \"v{{version}}\"\nlayout  = \"archive\"\nasset   = \"ciginstaller-v{{version}}-{{os}}-{{arch}}.tar.gz\"\nsidecar = \"{{asset}}.sha256\"\nstrip   = 0\nshims   = []\ntargets = [ \"{host}\" ]\nattests = false\n\n"
        ),
        other => binary(other, &format!("otmof-ops/{other}")),
    }
}

/// `Command::output`, tried again while exec fails with "text file busy": a
/// file written a moment ago can still be open in a child another test thread
/// forked, until that child execs.
pub fn output_retrying(c: &mut Command) -> std::io::Result<Output> {
    let mut tries = 0;
    loop {
        match c.output() {
            Err(e) if e.raw_os_error() == Some(26) && tries < 100 => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            r => return r,
        }
    }
}
