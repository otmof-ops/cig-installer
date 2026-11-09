// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0
//! Fake products: a `cig` shell script, a `cigo` tarball, a `cigcompile` script and a `ciginstaller`
//! tarball, each published with a real `.sha256` sidecar, plus the faults a test can inject.

use crate::sha256_hex;
use crate::tarball::TarGz;

/// `linux-x86_64` style name of the machine running the tests (what `Target::host()` gives).
pub fn host_target() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

fn exe_suffix() -> &'static str {
    if std::env::consts::OS == "windows" {
        ".exe"
    } else {
        ""
    }
}

/// The published file name of a product's asset for this machine (section 6.1's templates).
pub fn asset_name(tool: &str, version: &str) -> String {
    let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
    match tool {
        "cigo" => format!("cig-orchestrator-v{version}.tar.gz"),
        "ciginstaller" => format!("ciginstaller-v{version}-{os}-{arch}.tar.gz"),
        other => format!("{other}-v{version}-{os}-{arch}{}", exe_suffix()),
    }
}

/// How a product's published files are made wrong.
#[derive(Debug, Clone, Default)]
pub(crate) struct Faults {
    pub corrupt_asset: bool,
    pub bad_sidecar: bool,
    pub missing_sidecar: bool,
    pub truncate: bool,
    pub zip_slip: bool,
}

/// One fake product version. Built by `Fixture::product`; methods chain.
#[derive(Debug, Clone)]
pub struct ProductBuilder {
    pub tool: String,
    pub version: String,
    pub(crate) faults: Faults,
    pub(crate) requires: Option<String>,
    pub(crate) yanked: Option<String>,
    pub(crate) dry_run_output: Option<String>,
    pub(crate) selfcheck_fails: bool,
    pub(crate) extra_files: Vec<(String, Vec<u8>)>,
}

/// A product ready to publish: the bytes as served, and what the manifest will claim.
#[derive(Debug, Clone)]
pub(crate) struct Built {
    pub name: String,
    /// Bytes put in `releases/` (after any fault).
    pub served: Vec<u8>,
    /// What the manifest says: digest and size of the *intended* bytes.
    pub sha256: String,
    pub size: u64,
    /// `None` for `missing_sidecar`.
    pub sidecar: Option<String>,
}

impl ProductBuilder {
    pub(crate) fn new(tool: &str, version: &str) -> ProductBuilder {
        ProductBuilder {
            tool: tool.into(),
            version: version.into(),
            faults: Faults::default(),
            requires: None,
            yanked: None,
            dry_run_output: None,
            selfcheck_fails: false,
            extra_files: Vec::new(),
        }
    }

    /// One byte of the published asset is changed after the manifest and sidecar were computed:
    /// same size, wrong digest (E1821).
    pub fn corrupt_asset(&mut self) -> &mut Self {
        self.faults.corrupt_asset = true;
        self
    }
    /// The product's own `.sha256` names another digest; the manifest is right (E1827).
    pub fn bad_sidecar(&mut self) -> &mut Self {
        self.faults.bad_sidecar = true;
        self
    }
    /// No `.sha256` is published (E1827).
    pub fn missing_sidecar(&mut self) -> &mut Self {
        self.faults.missing_sidecar = true;
        self
    }
    /// The published asset is cut to half its length; manifest and sidecar describe the whole (E1821, size).
    pub fn truncate(&mut self) -> &mut Self {
        self.faults.truncate = true;
        self
    }
    /// An archive product carries a `../` entry. Manifest and sidecar describe the malicious archive
    /// truthfully, so it passes every digest and is stopped by `unpack_tar_gz` (E1826).
    pub fn zip_slip(&mut self) -> &mut Self {
        self.faults.zip_slip = true;
        self
    }
    /// Listed in the manifest's `[yanked]` as `"<tool> <version>"`.
    pub fn yank(&mut self, reason: &str) -> &mut Self {
        self.yanked = Some(reason.into());
        self
    }
    /// `[product.<tool>."<version>"] requires = { <tool> = "<range>" }`; give `"cig", ">=1.16.1, <2"`.
    pub fn requires(&mut self, tool: &str, range: &str) -> &mut Self {
        let r = format!("{tool} = \"{range}\"");
        self.requires = Some(match self.requires.take() {
            Some(prev) => format!("{prev}, {r}"),
            None => r,
        });
        self
    }
    /// What the fake `cig` prints for any invocation with `--dry-run` (the trial compares this).
    pub fn dry_run_output(&mut self, text: &str) -> &mut Self {
        self.dry_run_output = Some(text.into());
        self
    }
    /// The fake `ciginstaller`'s `__selfcheck` exits 1.
    pub fn selfcheck_fails(&mut self) -> &mut Self {
        self.selfcheck_fails = true;
        self
    }
    /// An extra file in an archive product (path inside the top directory).
    pub fn file(&mut self, path: &str, bytes: &[u8]) -> &mut Self {
        self.extra_files.push((path.into(), bytes.to_vec()));
        self
    }

    pub fn asset_name(&self) -> String {
        asset_name(&self.tool, &self.version)
    }

    pub fn is_archive(&self) -> bool {
        matches!(self.tool.as_str(), "cigo" | "ciginstaller")
    }

    fn cig_script(&self) -> Vec<u8> {
        let v = &self.version;
        let dry = match &self.dry_run_output {
            Some(t) => format!(
                "case \" $* \" in *\" --dry-run \"*) printf '%s\\n' '{}'; exit \"${{FAKE_EXIT:-0}}\" ;; esac\n",
                t.replace('\'', "'\\''")
            ),
            None => String::new(),
        };
        format!(
            "#!/bin/sh\n# fake {tool} {v} (cigi-testkit)\nif [ \"$1\" = \"--version\" ] || [ \"$1\" = \"version\" ]; then echo \"{tool} {v}\"; exit 0; fi\n{dry}echo \"{tool} {v} args=$* carton=${{CIGI_CARTON:-}}\"\nexit \"${{FAKE_EXIT:-0}}\"\n",
            tool = self.tool
        )
        .into_bytes()
    }

    fn archive(&self) -> Vec<u8> {
        let v = &self.version;
        let mut t = TarGz::new();
        if self.faults.zip_slip {
            t.file("x/ok.txt", b"fine", false);
            t.raw_file("x/../../escape.txt", b"owned", None);
            return t.finish();
        }
        match self.tool.as_str() {
            "cigo" => {
                let top = format!("cig-orchestrator-v{v}");
                t.dir(&format!("{top}/"));
                t.file(
                    &format!("{top}/bin/cigo"),
                    format!("#!/bin/sh\necho \"cigo {v} uses $(cig --version)\"\nexit \"${{FAKE_EXIT:-0}}\"\n").as_bytes(),
                    true,
                );
                t.file(
                    &format!("{top}/orchestrate.cig"),
                    format!("# orchestrate for cigo {v}\n").as_bytes(),
                    false,
                );
                // identical bytes in every version, so store dedup is measurable
                t.file(&format!("{top}/LICENSE"), b"Apache-2.0 (fixture)\n", false);
                t.file(
                    &format!("{top}/docs/a.md"),
                    b"# fixture docs, the same in every version\n",
                    false,
                );
                for (p, b) in &self.extra_files {
                    t.file(&format!("{top}/{p}"), b, false);
                }
            }
            _ => {
                let check = if self.selfcheck_fails {
                    "exit 1"
                } else {
                    "echo \"ciginstaller $VER\"; exit 0"
                };
                t.file(
                    "ciginstaller",
                    format!(
                        "#!/bin/sh\nVER={v}\nif [ \"$1\" = \"__selfcheck\" ]; then {check}; fi\necho \"ciginstaller $VER args=$*\"\n"
                    )
                    .as_bytes(),
                    true,
                );
                t.file(
                    "cigi-shim",
                    b"#!/bin/sh\n# fixture shim object, the same in every version\nexit 0\n",
                    true,
                );
                for (p, b) in &self.extra_files {
                    t.file(p, b, false);
                }
            }
        }
        t.finish()
    }

    /// The intended (fault-free) bytes of the asset.
    pub(crate) fn intended_bytes(&self) -> Vec<u8> {
        if self.is_archive() {
            self.archive()
        } else {
            self.cig_script()
        }
    }

    pub(crate) fn build(&self) -> Built {
        let name = self.asset_name();
        // zip_slip describes the malicious archive truthfully; every other fault lies about the served bytes
        let intended = self.intended_bytes();
        let sha256 = sha256_hex(&intended);
        let size = intended.len() as u64;
        let mut served = intended.clone();
        if self.faults.corrupt_asset {
            if let Some(b) = served.last_mut() {
                *b ^= 0x01;
            }
            if served.is_empty() {
                served.push(1);
            }
        }
        if self.faults.truncate {
            served.truncate(intended.len() / 2);
        }
        let sidecar = if self.faults.missing_sidecar {
            None
        } else if self.faults.bad_sidecar {
            Some(format!("{}  {name}\n", sha256_hex(b"some other file")))
        } else {
            Some(format!("{sha256}  {name}\n"))
        };
        Built {
            name,
            served,
            sha256,
            size,
            sidecar,
        }
    }
}
