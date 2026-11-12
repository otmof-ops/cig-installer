// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `cargo xtask carton-local`: a carton built from local source trees, with nothing published.
//!
//! Each product given a tree is built for one target in the shape its release workflow
//! publishes: the same file name, the same archive layout, and a `.sha256` sidecar in
//! `sha256sum`'s format. The files land flat in one directory, the way a mirror is laid out.
//! The manifest is then written by `assemble` itself, reading that directory through
//! [`DirReleases`]: there is one manifest writer, and the directory is its release host.
//! `ciginstaller install carton <v> --from <dir>` reads the result.
//!
//! The products depend on each other at build time. A release gets that through pins; a local
//! carton gets it from the sibling trees, with cargo patch overrides on the command line
//! (`--config patch...`), never by editing a tree: cigar is built against the local cig-script,
//! and cigcompile against the local cig-script and cigar-script. A patched build changes the
//! lock file, so it is built from a copy of its tree (target/ and .git left out) without
//! `--locked`; the unpatched builds use the tree in place, as their release workflows do.
//!
//! The archives are made with the same `tar` and `gzip` commands the release workflows run, so
//! this command needs GNU tar (Linux).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use cig_compat::{ToolDef, Version};
use sha2::{Digest, Sha256};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::assemble::{
    self, sidecar_line, Assembled, AssetInfo, ReleaseInfo, Releases, Request, TagInfo,
};

/// The products a local carton can hold, in build order.
pub const TOOLS: [&str; 5] = ["cig", "cigo", "cigcompile", "cigar", "ciginstaller"];

/// The Rust target each release workflow builds, and the manifest's name for that machine.
pub const TARGETS: [(&str, &str); 8] = [
    ("x86_64-unknown-linux-musl", "linux-x86_64"),
    ("aarch64-unknown-linux-musl", "linux-aarch64"),
    ("armv7-unknown-linux-musleabihf", "linux-arm"),
    ("aarch64-apple-darwin", "macos-aarch64"),
    ("x86_64-apple-darwin", "macos-x86_64"),
    ("x86_64-pc-windows-msvc", "windows-x86_64"),
    ("aarch64-pc-windows-msvc", "windows-aarch64"),
    ("x86_64-unknown-freebsd", "freebsd-x86_64"),
];

/// The manifest's target name of a Rust target the release workflows build.
pub fn manifest_target(triple: &str) -> Option<&'static str> {
    TARGETS.iter().find(|(t, _)| *t == triple).map(|(_, m)| *m)
}

/// The Rust target the release workflows build for the machine this runs on.
pub fn host_triple() -> Option<&'static str> {
    let name = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
    TARGETS.iter().find(|(_, m)| *m == name).map(|(t, _)| *t)
}

/// A directory of local builds, read as release hosts: every regular file in it is an asset of
/// each release `tags` names. The digest a host reports for an asset is the sha256 of the file
/// itself, so `assemble` checks every sidecar against the bytes beside it.
pub struct DirReleases {
    /// Each repository's one local release tag.
    tags: BTreeMap<String, String>,
    /// `YYYY-MM-DD`.
    published: String,
    /// Name, path, size and sha256 of each file, by name.
    files: Vec<(String, PathBuf, u64, String)>,
}

impl DirReleases {
    /// Read `dir` once. Names that start with a dot (a partial file) are not assets.
    pub fn scan(
        dir: &Path,
        tags: BTreeMap<String, String>,
        published: &str,
    ) -> Result<DirReleases, String> {
        let mut files = Vec::new();
        let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("{}: {e}", dir.display()))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let kind = entry
                .file_type()
                .map_err(|e| format!("{}: {e}", entry.path().display()))?;
            if name.starts_with('.') || !kind.is_file() {
                continue;
            }
            let path = entry.path();
            let (size, sha) = hash_file(&path)?;
            files.push((name, path, size, sha));
        }
        files.sort();
        Ok(DirReleases {
            tags,
            published: published.to_string(),
            files,
        })
    }
}

impl Releases for DirReleases {
    fn release(&self, repo: &str, tag: &str) -> Result<ReleaseInfo, String> {
        if self.tags.get(repo).map(String::as_str) != Some(tag) {
            return Err(format!("no local build of {repo} {tag} in this directory"));
        }
        let assets = self
            .files
            .iter()
            .enumerate()
            .map(|(i, (name, _, size, sha))| {
                (
                    name.clone(),
                    AssetInfo {
                        id: i as u64 + 1,
                        size: *size,
                        digest: Some(sha.clone()),
                    },
                )
            })
            .collect();
        Ok(ReleaseInfo {
            published: self.published.clone(),
            prerelease: false,
            assets,
            notes: String::new(),
        })
    }

    fn small_asset(&self, _repo: &str, asset: &AssetInfo, max: u64) -> Result<Vec<u8>, String> {
        let (name, path, size, _) = asset
            .id
            .checked_sub(1)
            .and_then(|i| self.files.get(i as usize))
            .ok_or_else(|| format!("no asset {} in this directory", asset.id))?;
        if *size > max {
            return Err(format!("{name} is {size} bytes, over {max}"));
        }
        std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
    }

    fn tags(&self, repo: &str) -> Result<Vec<TagInfo>, String> {
        Ok(self
            .tags
            .get(repo)
            .map(|tag| TagInfo {
                tag: tag.clone(),
                prerelease: false,
            })
            .into_iter()
            .collect())
    }
}

fn hash_file(path: &Path) -> Result<(u64, String), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok((bytes.len() as u64, hex::encode(Sha256::digest(&bytes))))
}

/// The version in a `Cargo.toml`: `[package] version`, or the workspace's when the package
/// inherits it or the file is a virtual workspace.
pub fn cargo_version(text: &str) -> Result<Version, String> {
    let doc: toml::Table = text.parse().map_err(|e| format!("Cargo.toml: {e}"))?;
    let workspace = doc
        .get("workspace")
        .and_then(|w| w.get("package"))
        .and_then(|p| p.get("version"))
        .and_then(toml::Value::as_str);
    let own = doc.get("package").and_then(|p| p.get("version"));
    let v = match own {
        Some(toml::Value::String(s)) => s.as_str(),
        Some(_) | None => workspace.ok_or("Cargo.toml states no version")?,
    };
    Version::parse(v).map_err(|e| format!("Cargo.toml version `{v}`: {e}"))
}

/// cigo's version: `stick VERSION = "X.Y.Z"` in `orchestrate.cig`, the line its release
/// workflow checks against the tag.
pub fn cigo_version(text: &str) -> Result<Version, String> {
    let v = text
        .lines()
        .find_map(|l| l.strip_prefix("stick VERSION = \""))
        .and_then(|rest| rest.split_once('"'))
        .map(|(v, _)| v)
        .ok_or("orchestrate.cig has no `stick VERSION = \"...\"` line")?;
    Version::parse(v).map_err(|e| format!("orchestrate.cig VERSION `{v}`: {e}"))
}

/// What to build.
#[derive(Clone, Debug)]
pub struct Options {
    /// Each product's source tree, by tool name. A tool with no tree is not in the carton.
    pub trees: BTreeMap<String, PathBuf>,
    pub carton: Version,
    /// The carton directory: empty or missing.
    pub out: PathBuf,
    /// Build output, kept between runs: `<work>/<tool>/target`, and the copies of patched trees.
    pub work: PathBuf,
    /// A Rust target from [`TARGETS`].
    pub triple: String,
    pub channel: String,
    pub expires_days: i64,
    /// `manifest/tools.toml`.
    pub tools_toml: String,
    /// `manifest/products.toml`.
    pub products_toml: String,
    pub now: OffsetDateTime,
}

/// One product built.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Built {
    pub tool: String,
    pub version: Version,
    pub asset: String,
}

/// The summary file beside the manifest: `KEY=value` lines, read by `ci/carton-gate.sh`.
pub const SUMMARY: &str = "carton-local.env";

/// Build every product given a tree, write the manifest, and return what `assemble` wrote.
pub fn run(o: &Options, log: &mut dyn FnMut(&str)) -> Result<(Assembled, Vec<Built>), String> {
    let target = manifest_target(&o.triple).ok_or_else(|| {
        format!(
            "{} is not a target the release workflows build ({})",
            o.triple,
            TARGETS.map(|(t, _)| t).join(", ")
        )
    })?;
    if o.trees.is_empty() {
        return Err("give at least one product tree".into());
    }
    for tool in o.trees.keys() {
        if !TOOLS.contains(&tool.as_str()) {
            return Err(format!(
                "{tool} is not a product (one of {})",
                TOOLS.join(", ")
            ));
        }
    }
    if o.out.exists() {
        let mut d = std::fs::read_dir(&o.out).map_err(|e| format!("{}: {e}", o.out.display()))?;
        if d.next().is_some() {
            return Err(format!(
                "{} is not empty; a local carton goes in an empty directory",
                o.out.display()
            ));
        }
    }
    std::fs::create_dir_all(&o.out).map_err(|e| format!("{}: {e}", o.out.display()))?;
    std::fs::create_dir_all(&o.work).map_err(|e| format!("{}: {e}", o.work.display()))?;
    let mut trees = BTreeMap::new();
    for (tool, t) in &o.trees {
        let abs = std::fs::canonicalize(t).map_err(|e| format!("{}: {e}", t.display()))?;
        trees.insert(tool.clone(), abs);
    }
    let out = std::fs::canonicalize(&o.out).map_err(|e| format!("{}: {e}", o.out.display()))?;
    let work = std::fs::canonicalize(&o.work).map_err(|e| format!("{}: {e}", o.work.display()))?;

    let now = o.now.replace_nanosecond(0).unwrap_or(o.now);
    let stamp = now.format(&Rfc3339).map_err(|e| e.to_string())?;
    let mut req = Request {
        channel: o.channel.clone(),
        carton: o.carton.clone(),
        members: BTreeMap::new(),
        packs: BTreeMap::new(),
        notes: format!(
            "A local carton built for {target} by cargo xtask carton-local; not published."
        ),
        expires_days: o.expires_days,
        previous: None,
        tools_toml: o.tools_toml.clone(),
        products_toml: o.products_toml.clone(),
        now,
    };
    let defs = tool_defs(&req)?;
    let b = Builder {
        triple: &o.triple,
        work: &work,
        trees: &trees,
        defs: &defs,
    };

    let mut tags = BTreeMap::new();
    let mut built = Vec::new();
    for tool in TOOLS {
        let Some(tree) = trees.get(tool) else {
            log(&format!(
                "{tool}: no tree given, so it is not in this carton"
            ));
            continue;
        };
        let def = defs
            .get(tool)
            .ok_or_else(|| format!("manifest/tools.toml defines no tool.{tool}"))?;
        let version = match tool {
            "cigo" => cigo_version(&read(&tree.join("orchestrate.cig"))?)?,
            _ => cargo_version(&read(&tree.join("Cargo.toml"))?)?,
        };
        let for_target = if def.targets.iter().any(|t| t == "any") {
            "any"
        } else if def.targets.iter().any(|t| t == target) {
            target
        } else {
            return Err(format!(
                "tool.{tool} is not released for {target} (its targets: {})",
                def.targets.join(", ")
            ));
        };
        let names = def
            .names(&version, for_target)
            .map_err(|e| format!("tool.{tool}: {e}"))?;
        log(&format!("{tool} {version}: building {}", names.asset));
        let dest = out.join(&names.asset);
        match tool {
            "cig" => b.cig(tree, &dest)?,
            "cigo" => b.cigo(tree, &names.asset, &dest)?,
            "cigcompile" => b.cigcompile(tree, &dest)?,
            "cigar" => b.cigar(tree, &dest)?,
            _ => b.installer(tree, &dest)?,
        }
        let (_, sha) = hash_file(&dest)?;
        write(&out.join(&names.sidecar), &sidecar_line(&sha, &names.asset))?;
        log(&format!("{tool} {version}: {}  {sha}", names.asset));
        tags.insert(def.repo.clone(), names.tag);
        built.push(Built {
            tool: tool.to_string(),
            version,
            asset: names.asset,
        });
    }

    req.members = built
        .iter()
        .map(|b| (b.tool.clone(), b.version.clone()))
        .collect();
    let rel = DirReleases::scan(&out, tags, &stamp[..10])?;
    let done = assemble::assemble(&req, &rel)?;
    let name = format!("manifest-{}.toml", o.channel);
    write(&out.join(&name), &done.text)?;
    write(
        &out.join(format!("{name}.sha256")),
        &sidecar_line(&done.sha256, &name),
    )?;
    write(&out.join(SUMMARY), &summary(o, target, &built))?;
    Ok((done, built))
}

/// The tool definitions of `manifest/tools.toml`, through the client's parser.
pub fn tool_defs(req: &Request) -> Result<BTreeMap<String, ToolDef>, String> {
    let fmt = |t: OffsetDateTime| t.format(&Rfc3339).map_err(|e| e.to_string());
    let now = req.now.replace_nanosecond(0).unwrap_or(req.now);
    let (generated, expires) = (fmt(now)?, fmt(now + time::Duration::days(1))?);
    Ok(assemble::definitions(req, &generated, &expires)?.tools)
}

/// The `KEY=value` lines of [`SUMMARY`].
pub fn summary(o: &Options, target: &str, built: &[Built]) -> String {
    let mut s = format!(
        "CARTON={}\nCHANNEL={}\nTARGET={target}\nTRIPLE={}\n",
        o.carton, o.channel, o.triple
    );
    for b in built {
        let key = b.tool.to_ascii_uppercase();
        s.push_str(&format!("{key}={}\n{key}_ASSET={}\n", b.version, b.asset));
    }
    s
}

fn read(p: &Path) -> Result<String, String> {
    std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))
}

fn write(p: &Path, text: &str) -> Result<(), String> {
    std::fs::write(p, text).map_err(|e| format!("{}: {e}", p.display()))
}

/// A cargo `--config` value that points a git source's crate at a local tree.
pub fn patch_arg(git_url: &str, krate: &str, tree: &Path) -> String {
    let path = toml::Value::String(tree.to_string_lossy().into_owned());
    format!("patch.\"{git_url}\".{krate}.path={path}")
}

struct Builder<'a> {
    triple: &'a str,
    work: &'a Path,
    trees: &'a BTreeMap<String, PathBuf>,
    defs: &'a BTreeMap<String, ToolDef>,
}

impl Builder<'_> {
    fn exe(&self) -> &'static str {
        if self.triple.contains("windows") {
            ".exe"
        } else {
            ""
        }
    }

    fn target_dir(&self, tool: &str) -> PathBuf {
        self.work.join(tool).join("target")
    }

    fn release_dir(&self, tool: &str) -> PathBuf {
        self.target_dir(tool).join(self.triple).join("release")
    }

    /// `cargo <args> --target <triple>` in `dir`, with this tool's own target directory. On a
    /// musl target the C compiler is `musl-gcc` unless one is set, as the release workflows do.
    fn cargo(&self, tool: &str, dir: &Path, args: &[String]) -> Result<(), String> {
        let mut c = Command::new("cargo");
        c.current_dir(dir)
            .args(args)
            .arg("--target")
            .arg(self.triple)
            .env("CARGO_TARGET_DIR", self.target_dir(tool))
            // the tree's own rust-toolchain.toml picks the toolchain, not the one running xtask
            .env_remove("RUSTUP_TOOLCHAIN")
            .env_remove("CARGO");
        if self.triple.contains("-linux-musl") {
            let var = format!("CC_{}", self.triple.replace('-', "_"));
            if std::env::var_os(&var).is_none() {
                c.env(var, "musl-gcc");
            }
        }
        run_cmd(
            &mut c,
            &format!("cargo {} in {}", args.join(" "), dir.display()),
        )
    }

    /// The `--config` patches for `tool`'s build: the sibling trees given.
    fn patches(&self, wants: &[(&str, &str)]) -> Vec<String> {
        let mut args = Vec::new();
        for (tool, krate) in wants {
            if let (Some(tree), Some(def)) = (self.trees.get(*tool), self.defs.get(*tool)) {
                args.push("--config".to_string());
                args.push(patch_arg(
                    &format!("https://github.com/{}", def.repo),
                    krate,
                    tree,
                ));
            }
        }
        args
    }

    /// A copy of `tree` to build with patches: the patch rewrites the lock file, and the tree
    /// itself is never written.
    fn copy(&self, tool: &str, tree: &Path) -> Result<PathBuf, String> {
        let to = self.work.join(tool).join("src");
        if to.exists() {
            std::fs::remove_dir_all(&to).map_err(|e| format!("{}: {e}", to.display()))?;
        }
        copy_tree(tree, &to, true)?;
        Ok(to)
    }

    fn cig(&self, tree: &Path, dest: &Path) -> Result<(), String> {
        self.cargo("cig", tree, &strs(&["build", "--release"]))?;
        copy_file(
            &self.release_dir("cig").join(format!("cig{}", self.exe())),
            dest,
        )
    }

    fn cigcompile(&self, tree: &Path, dest: &Path) -> Result<(), String> {
        let src = self.copy("cigcompile", tree)?;
        let mut args = strs(&["build", "--release", "--bin", "cigcompile"]);
        args.extend(self.patches(&[("cig", "cigscript"), ("cigar", "cigarscript")]));
        self.cargo("cigcompile", &src, &args)?;
        copy_file(
            &self
                .release_dir("cigcompile")
                .join(format!("cigcompile{}", self.exe())),
            dest,
        )
    }

    fn cigar(&self, tree: &Path, dest: &Path) -> Result<(), String> {
        let src = self.copy("cigar", tree)?;
        let mut args = strs(&["build", "--release", "--bin", "cigar", "--bin", "cigar-lsp"]);
        args.extend(self.patches(&[("cig", "cigscript")]));
        self.cargo("cigar", &src, &args)?;
        let stage = self.work.join("cigar").join("stage");
        fresh_dir(&stage.join("bin"))?;
        for bin in ["cigar", "cigar-lsp"] {
            let name = format!("{bin}{}", self.exe());
            copy_file(
                &self.release_dir("cigar").join(&name),
                &stage.join("bin").join(&name),
            )?;
        }
        let mut c = Command::new("tar");
        c.arg("-czf").arg(dest).arg("-C").arg(&stage).arg("bin");
        run_cmd(&mut c, "tar (cigar)")
    }

    fn cigo(&self, tree: &Path, asset: &str, dest: &Path) -> Result<(), String> {
        let top = asset
            .strip_suffix(".tar.gz")
            .ok_or_else(|| format!("tool.cigo's asset {asset} is not a .tar.gz"))?;
        let stage = self.work.join("cigo").join("stage");
        fresh_dir(&stage.join(top).join("bin"))?;
        let dir = stage.join(top);
        for f in [
            "orchestrate.cig",
            "install.sh",
            "install.ps1",
            "LICENSE",
            "NOTICE",
            "README.md",
            "CHANGELOG.md",
            "TOOLCHAINS.md",
            "YANKED",
        ] {
            copy_file(&tree.join(f), &dir.join(f))?;
        }
        for f in ["cigo", "cigo.launch.ps1", "cigo.cmd"] {
            copy_file(&tree.join("bin").join(f), &dir.join("bin").join(f))?;
        }
        for d in ["LICENSES", "docs"] {
            copy_tree(&tree.join(d), &dir.join(d), false)?;
        }
        let mut c = Command::new("tar");
        c.arg("-czf").arg(dest).arg("-C").arg(&stage).arg(top);
        run_cmd(&mut c, "tar (cigo)")
    }

    fn installer(&self, tree: &Path, dest: &Path) -> Result<(), String> {
        self.cargo(
            "ciginstaller",
            tree,
            &strs(&[
                "build",
                "--release",
                "--locked",
                "-p",
                "ciginstaller",
                "-p",
                "cigi-shim",
            ]),
        )?;
        let stage = self.work.join("ciginstaller").join("stage");
        fresh_dir(&stage)?;
        let rel = self.release_dir("ciginstaller");
        for bin in ["ciginstaller", "cigi-shim"] {
            let name = format!("{bin}{}", self.exe());
            copy_file(&rel.join(&name), &stage.join(&name))?;
        }
        copy_file(&tree.join("LICENSE"), &stage.join("LICENSE"))?;
        copy_file(&tree.join("NOTICE"), &stage.join("NOTICE"))?;
        copy_file(
            &tree.join("THIRD-PARTY-NOTICES.md"),
            &stage.join("THIRD-PARTY-NOTICES"),
        )?;
        // the release workflow's own flags; the commit time is not known in a tree without .git
        let mtime = std::env::var("SOURCE_DATE_EPOCH").unwrap_or_else(|_| "0".into());
        let tar = self.work.join("ciginstaller").join("ciginstaller.tar");
        let _ = std::fs::remove_file(&tar);
        let _ = std::fs::remove_file(tar.with_extension("tar.gz"));
        let mut c = Command::new("tar");
        c.args(["--sort=name", "--owner=0", "--group=0", "--numeric-owner"])
            .arg(format!("--mtime=@{mtime}"))
            .arg("-C")
            .arg(&stage)
            .arg("-cf")
            .arg(&tar)
            .arg(".");
        run_cmd(&mut c, "tar (ciginstaller)")?;
        run_cmd(Command::new("gzip").arg("-n").arg(&tar), "gzip -n")?;
        let gz = tar.with_extension("tar.gz");
        std::fs::rename(&gz, dest).map_err(|e| format!("{}: {e}", dest.display()))
    }
}

fn strs(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

fn run_cmd(c: &mut Command, what: &str) -> Result<(), String> {
    let status = c
        .status()
        .map_err(|e| format!("{what}: cannot start it: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{what}: {status}"))
    }
}

fn fresh_dir(p: &Path) -> Result<(), String> {
    if p.exists() {
        std::fs::remove_dir_all(p).map_err(|e| format!("{}: {e}", p.display()))?;
    }
    std::fs::create_dir_all(p).map_err(|e| format!("{}: {e}", p.display()))
}

/// Copy one file, its permissions with it.
fn copy_file(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::copy(from, to)
        .map(|_| ())
        .map_err(|e| format!("{} to {}: {e}", from.display(), to.display()))
}

/// Copy a directory tree. With `skip_build`, the top-level `target` and `.git` stay behind.
/// A symbolic link is copied as a link.
fn copy_tree(from: &Path, to: &Path, skip_build: bool) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| format!("{}: {e}", to.display()))?;
    let entries = std::fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("{}: {e}", from.display()))?;
        let name = entry.file_name();
        if skip_build && (name == "target" || name == ".git") {
            continue;
        }
        let (src, dst) = (entry.path(), to.join(&name));
        let kind = entry
            .file_type()
            .map_err(|e| format!("{}: {e}", src.display()))?;
        if kind.is_dir() {
            copy_tree(&src, &dst, false)?;
        } else if kind.is_symlink() {
            let link = std::fs::read_link(&src).map_err(|e| format!("{}: {e}", src.display()))?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(&link, &dst)
                .map_err(|e| format!("{}: {e}", dst.display()))?;
            #[cfg(not(unix))]
            return Err(format!(
                "{} is a link to {}; copying links needs Unix",
                src.display(),
                link.display()
            ));
        } else {
            copy_file(&src, &dst)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOOLS_TOML: &str = include_str!("../../manifest/tools.toml");
    const PRODUCTS_TOML: &str = include_str!("../../manifest/products.toml");

    fn put(dir: &Path, name: &str, bytes: &[u8]) -> String {
        std::fs::write(dir.join(name), bytes).unwrap();
        let d = hex::encode(Sha256::digest(bytes));
        std::fs::write(dir.join(format!("{name}.sha256")), sidecar_line(&d, name)).unwrap();
        d
    }

    fn tags() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("otmof-ops/cig-script".to_string(), "v1.21.0".to_string()),
            (
                "otmof-ops/cig-orchestrator".to_string(),
                "v1.19.0".to_string(),
            ),
        ])
    }

    fn request() -> Request {
        Request {
            channel: "stable".into(),
            carton: Version::parse("0.2.0").unwrap(),
            members: BTreeMap::from([
                ("cig".to_string(), Version::parse("1.21.0").unwrap()),
                ("cigo".to_string(), Version::parse("1.19.0").unwrap()),
            ]),
            packs: BTreeMap::new(),
            notes: String::new(),
            expires_days: 30,
            previous: None,
            tools_toml: TOOLS_TOML.into(),
            products_toml: PRODUCTS_TOML.into(),
            now: OffsetDateTime::from_unix_timestamp(1_794_000_000).unwrap(),
        }
    }

    #[test]
    fn a_directory_of_local_builds_feeds_assemble() {
        let t = tempfile::tempdir().unwrap();
        let cig = put(t.path(), "cig-v1.21.0-linux-x86_64", b"a local cig");
        let cigo = put(t.path(), "cig-orchestrator-v1.19.0.tar.gz", b"a local cigo");
        std::fs::write(t.path().join(".partial"), b"not an asset").unwrap();
        let rel = DirReleases::scan(t.path(), tags(), "2026-11-12").unwrap();
        let done = assemble::assemble(&request(), &rel).unwrap();
        assert!(done.text.contains(&cig), "{}", done.text);
        assert!(done.text.contains(&cigo), "{}", done.text);
        let m = &done.manifest;
        let c = m.carton(&Version::parse("0.2.0").unwrap()).unwrap();
        assert_eq!(c.products.len(), 2);
        let b = &m.builds[&("cig".to_string(), Version::parse("1.21.0").unwrap())];
        assert_eq!(format!("{b:?}").matches(&cig).count(), 1, "{b:?}");
        assert!(
            done.report.iter().any(|l| l.contains("no build for")),
            "only linux-x86_64 is here: {:?}",
            done.report
        );
    }

    #[test]
    fn a_sidecar_that_disagrees_with_the_file_is_refused() {
        let t = tempfile::tempdir().unwrap();
        put(t.path(), "cig-v1.21.0-linux-x86_64", b"a local cig");
        put(t.path(), "cig-orchestrator-v1.19.0.tar.gz", b"a local cigo");
        std::fs::write(t.path().join("cig-v1.21.0-linux-x86_64"), b"other bytes").unwrap();
        let rel = DirReleases::scan(t.path(), tags(), "2026-11-12").unwrap();
        let e = assemble::assemble(&request(), &rel).unwrap_err();
        assert!(e.contains("GitHub's own digest"), "{e}");
    }

    #[test]
    fn a_release_the_directory_does_not_hold_is_an_error() {
        let t = tempfile::tempdir().unwrap();
        put(t.path(), "cig-v1.21.0-linux-x86_64", b"a local cig");
        let rel = DirReleases::scan(t.path(), tags(), "2026-11-12").unwrap();
        assert!(rel.release("otmof-ops/cig-script", "v1.20.0").is_err());
        assert!(rel.release("otmof-ops/cig-compile", "v0.4.0").is_err());
        assert_eq!(
            rel.tags("otmof-ops/cig-script").unwrap(),
            vec![TagInfo {
                tag: "v1.21.0".into(),
                prerelease: false
            }]
        );
        assert!(rel.tags("otmof-ops/cig-compile").unwrap().is_empty());
    }

    #[test]
    fn a_small_asset_is_refused_above_its_limit() {
        let t = tempfile::tempdir().unwrap();
        put(t.path(), "cig-v1.21.0-linux-x86_64", b"a local cig");
        let rel = DirReleases::scan(t.path(), tags(), "2026-11-12").unwrap();
        let r = rel.release("otmof-ops/cig-script", "v1.21.0").unwrap();
        let side = &r.assets["cig-v1.21.0-linux-x86_64.sha256"];
        let text = rel.small_asset("otmof-ops/cig-script", side, 4096).unwrap();
        assert!(String::from_utf8(text)
            .unwrap()
            .ends_with("  cig-v1.21.0-linux-x86_64\n"));
        assert!(rel.small_asset("otmof-ops/cig-script", side, 10).is_err());
        let bin = &r.assets["cig-v1.21.0-linux-x86_64"];
        assert_eq!(bin.size, 11);
        assert_eq!(
            bin.digest.as_deref(),
            Some(hex::encode(Sha256::digest(b"a local cig")).as_str())
        );
    }

    #[test]
    fn versions_are_read_where_each_release_workflow_reads_them() {
        let pkg = "[package]\nname = \"cigscript\"\nversion = \"1.21.0\"\n";
        assert_eq!(cargo_version(pkg).unwrap().to_string(), "1.21.0");
        let ws = "[workspace.package]\nversion = \"0.1.0\"\n[package]\nname = \"x\"\nversion.workspace = true\n";
        assert_eq!(cargo_version(ws).unwrap().to_string(), "0.1.0");
        let virt = "[workspace]\nmembers = []\n[workspace.package]\nversion = \"0.1.0\"\n";
        assert_eq!(cargo_version(virt).unwrap().to_string(), "0.1.0");
        assert!(cargo_version("[package]\nname = \"x\"\n").is_err());
        let orch = "# cig >=1.20.0\nstick VERSION = \"1.19.0\"\nstick VERSIONS = []\n";
        assert_eq!(cigo_version(orch).unwrap().to_string(), "1.19.0");
        assert!(cigo_version("stick NAME = \"cigo\"\n").is_err());
    }

    #[test]
    fn targets_are_the_release_workflows_names() {
        assert_eq!(
            manifest_target("x86_64-unknown-linux-musl"),
            Some("linux-x86_64")
        );
        assert_eq!(
            manifest_target("aarch64-pc-windows-msvc"),
            Some("windows-aarch64")
        );
        assert_eq!(manifest_target("x86_64-unknown-linux-gnu"), None);
        if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            assert_eq!(host_triple(), Some("x86_64-unknown-linux-musl"));
        }
        for (_, m) in TARGETS {
            assert!(
                TOOLS_TOML.contains(&format!("\"{m}\"")),
                "{m} is not a target of manifest/tools.toml"
            );
        }
    }

    #[test]
    fn the_definitions_name_each_products_release_assets() {
        let defs = tool_defs(&request()).unwrap();
        let v = |s: &str| Version::parse(s).unwrap();
        let n = |t: &str, ver: &str, target: &str| defs[t].names(&v(ver), target).unwrap().asset;
        assert_eq!(
            n("cig", "1.21.0", "linux-x86_64"),
            "cig-v1.21.0-linux-x86_64"
        );
        assert_eq!(
            n("cigo", "1.19.0", "any"),
            "cig-orchestrator-v1.19.0.tar.gz"
        );
        assert_eq!(
            n("cigcompile", "0.4.0", "linux-x86_64"),
            "cigcompile-v0.4.0-linux-x86_64"
        );
        assert_eq!(
            n("cigar", "0.1.0", "linux-x86_64"),
            "cigar-v0.1.0-linux-x86_64.tar.gz"
        );
        assert_eq!(
            n("ciginstaller", "0.1.0", "linux-x86_64"),
            "ciginstaller-v0.1.0-linux-x86_64.tar.gz"
        );
    }

    #[test]
    fn a_patch_names_the_git_source_and_quotes_the_path() {
        let a = patch_arg(
            "https://github.com/otmof-ops/cig-script",
            "cigscript",
            Path::new("/w/cig \"script\""),
        );
        // cargo reads a `--config` value as one line of TOML
        let doc: toml::Table = a.parse().unwrap();
        let path = doc["patch"]["https://github.com/otmof-ops/cig-script"]["cigscript"]["path"]
            .as_str()
            .unwrap();
        assert_eq!(path, "/w/cig \"script\"");
    }

    #[test]
    fn the_summary_names_the_carton_and_each_build() {
        let o = Options {
            trees: BTreeMap::new(),
            carton: Version::parse("0.2.0").unwrap(),
            out: PathBuf::new(),
            work: PathBuf::new(),
            triple: "x86_64-unknown-linux-musl".into(),
            channel: "stable".into(),
            expires_days: 30,
            tools_toml: String::new(),
            products_toml: String::new(),
            now: OffsetDateTime::UNIX_EPOCH,
        };
        let s = summary(
            &o,
            "linux-x86_64",
            &[Built {
                tool: "cigar".into(),
                version: Version::parse("0.1.0").unwrap(),
                asset: "cigar-v0.1.0-linux-x86_64.tar.gz".into(),
            }],
        );
        assert_eq!(
            s,
            "CARTON=0.2.0\nCHANNEL=stable\nTARGET=linux-x86_64\nTRIPLE=x86_64-unknown-linux-musl\n\
             CIGAR=0.1.0\nCIGAR_ASSET=cigar-v0.1.0-linux-x86_64.tar.gz\n"
        );
    }

    #[test]
    fn a_carton_directory_must_start_empty() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join("old"), b"x").unwrap();
        let o = Options {
            trees: BTreeMap::from([("cig".to_string(), t.path().to_path_buf())]),
            carton: Version::parse("0.2.0").unwrap(),
            out: t.path().to_path_buf(),
            work: t.path().join("work"),
            triple: "x86_64-unknown-linux-musl".into(),
            channel: "stable".into(),
            expires_days: 30,
            tools_toml: TOOLS_TOML.into(),
            products_toml: PRODUCTS_TOML.into(),
            now: OffsetDateTime::UNIX_EPOCH,
        };
        let e = run(&o, &mut |_| {}).unwrap_err();
        assert!(e.contains("not empty"), "{e}");
        let mut bad = o.clone();
        bad.triple = "x86_64-unknown-linux-gnu".into();
        assert!(run(&bad, &mut |_| {}).unwrap_err().contains("not a target"));
    }
}
