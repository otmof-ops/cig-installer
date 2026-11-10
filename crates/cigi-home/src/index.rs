// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! `installer/shim-index` (section 5.3): the one generated text file the shim
//! reads, and the rename that switches the machine to a new one.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use cig_compat::Version;

use crate::error::{JournalError, StateError};
use crate::home::Home;
use crate::journal::{FlipIntent, Journal, Txn, MAX_INDEX_BYTES};
use crate::state::State;
use crate::toolchain;

/// First line of the file.
pub const INDEX_HEADER: &str = "cigi-index 1";

/// Pin files that send the shim to the slow path (the list can grow without
/// a new shim).
pub const LEGACY_PIN_FILES: &[&str] = &[".cig-version", ".cigar-version"];

/// The content of `shim-index`, structured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShimIndex {
    /// The carton used when nothing pins one.
    pub machine: Option<String>,
    /// carton -> tool -> absolute path of the tool's real file.
    pub cartons: BTreeMap<String, BTreeMap<String, PathBuf>>,
    /// tool -> path: the fallback when the machine carton lacks the tool.
    pub sticks: BTreeMap<String, PathBuf>,
    pub legacy_pin_files: Vec<String>,
    /// Cartons the manifest yanks. Not rendered one by one: a yanked carton is
    /// never named by a `carton-minor`/`carton-major` line, so a pin like `0.1`
    /// does not run it in the shim while the resolver would pass it over.
    pub yanked: BTreeSet<String>,
}

impl Default for ShimIndex {
    fn default() -> ShimIndex {
        ShimIndex {
            machine: None,
            cartons: BTreeMap::new(),
            sticks: BTreeMap::new(),
            legacy_pin_files: LEGACY_PIN_FILES.iter().map(|s| s.to_string()).collect(),
            yanked: BTreeSet::new(),
        }
    }
}

fn ver(s: &str) -> Option<Version> {
    Version::parse(s).ok()
}

/// Order carton strings by version; anything unparseable sorts last, by text.
fn carton_order(a: &str, b: &str) -> std::cmp::Ordering {
    match (ver(a), ver(b)) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.cmp(b),
    }
}

impl ShimIndex {
    /// Build the index from `state` and the receipts on disk: for each carton
    /// and tool, the toolchain's shim target; sticks likewise. A tool whose
    /// toolchain is not installed is left out (the shim then reports it).
    pub fn from_state(home: &Home, state: &State) -> ShimIndex {
        let mut idx = ShimIndex {
            machine: state.machine_carton.clone(),
            ..ShimIndex::default()
        };
        let target = |tool: &str, version: &str| -> Option<PathBuf> {
            let v = ver(version)?;
            let r = toolchain::read_receipt(home, tool, &v).ok()?;
            let rel = r.shim_target(tool);
            if !cig_compat::is_plain_rel_path(&rel) {
                return None;
            }
            let mut p = home.toolchain(tool, &v);
            for part in rel.split('/') {
                p.push(part);
            }
            Some(p)
        };
        for (carton, rec) in &state.cartons {
            let mut tools = BTreeMap::new();
            for (tool, version) in &rec.tools {
                if let Some(p) = target(tool, version) {
                    tools.insert(tool.clone(), p);
                }
            }
            idx.cartons.insert(carton.clone(), tools);
        }
        for s in &state.sticks {
            if let Some(p) = target(&s.tool, &s.version) {
                // a rolled-back tool runs in place of the machine carton's own build
                if s.is_rollback() {
                    if let Some(m) = idx.machine.clone() {
                        idx.cartons
                            .entry(m)
                            .or_default()
                            .insert(s.tool.clone(), p.clone());
                    }
                }
                idx.sticks.insert(s.tool.clone(), p);
            }
        }
        idx
    }

    /// Set the yanked cartons (from the manifest known at write time).
    pub fn with_yanked<I: IntoIterator<Item = String>>(mut self, yanked: I) -> ShimIndex {
        self.yanked = yanked.into_iter().collect();
        self
    }

    /// The text of the file. Deterministic: cartons by version, tools by
    /// name, tab-separated, `carton-minor`/`carton-major` naming the newest
    /// non-prerelease, non-yanked installed carton of each series.
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(INDEX_HEADER);
        out.push('\n');
        if let Some(m) = &self.machine {
            out.push_str(&format!("machine\t{m}\n"));
        }
        let mut cartons: Vec<&String> = self.cartons.keys().collect();
        cartons.sort_by(|a, b| carton_order(a, b));
        for c in &cartons {
            for (tool, path) in &self.cartons[*c] {
                out.push_str(&format!("carton\t{c}\t{tool}\t{}\n", path.display()));
            }
        }
        for (tool, path) in &self.sticks {
            out.push_str(&format!("stick\t{tool}\t{}\n", path.display()));
        }
        let mut minors: BTreeMap<(u64, u64), &String> = BTreeMap::new();
        let mut majors: BTreeMap<u64, &String> = BTreeMap::new();
        for c in &cartons {
            if let Some(v) = ver(c) {
                if v.pre.is_some() || self.yanked.contains(*c) {
                    continue;
                }
                // `cartons` is ascending, so the last insert is the newest.
                minors.insert((v.major, v.minor), c);
                majors.insert(v.major, c);
            }
        }
        for ((ma, mi), full) in &minors {
            out.push_str(&format!("carton-minor\t{ma}.{mi}\t{full}\n"));
        }
        for (ma, full) in &majors {
            out.push_str(&format!("carton-major\t{ma}\t{full}\n"));
        }
        out.push_str("legacy-pin-files");
        for f in &self.legacy_pin_files {
            out.push('\t');
            out.push_str(f);
        }
        out.push('\n');
        out
    }

    /// Parse a rendered index. `None` when the header is not `cigi-index 1`.
    pub fn parse(text: &str) -> Option<ShimIndex> {
        let mut lines = text.lines();
        if lines.next()? != INDEX_HEADER {
            return None;
        }
        let mut idx = ShimIndex {
            legacy_pin_files: Vec::new(),
            ..ShimIndex::default()
        };
        for l in lines {
            let f: Vec<&str> = l.split('\t').collect();
            match f.as_slice() {
                ["machine", m] => idx.machine = Some(m.to_string()),
                ["carton", c, t, p] => {
                    idx.cartons
                        .entry(c.to_string())
                        .or_default()
                        .insert(t.to_string(), PathBuf::from(p));
                }
                ["stick", t, p] => {
                    idx.sticks.insert(t.to_string(), PathBuf::from(p));
                }
                ["legacy-pin-files", rest @ ..] => {
                    idx.legacy_pin_files = rest.iter().map(|s| s.to_string()).collect();
                }
                _ => {} // carton-minor/-major are derived; unknown lines ignored
            }
        }
        Some(idx)
    }

    /// What the fast shim does with a carton pin (step 5 of section 9):
    /// `pin` is the text of a `.cig-carton` (`0.1.0`, `0.1`, `0`, with or
    /// without `v`); `>=` pins and anything unparseable go to the slow path
    /// (`None`). No pin: the machine carton, then a stick. Returns the carton
    /// chosen (none for a stick) and the path. This is the reference the
    /// shim's tests compare against.
    pub fn pick(&self, pin: Option<&str>, tool: &str) -> Option<(Option<String>, PathBuf)> {
        let carton = match pin {
            Some(p) => {
                // cig-compat's `Pin::parse`: leading `v`s, then digits and dots
                // only. A prerelease carton cannot be pinned by a pin file, and
                // anything else is for the resolver.
                let p = p.trim().trim_start_matches('v');
                if p.is_empty()
                    || p.split('.')
                        .any(|x| x.is_empty() || !x.bytes().all(|b| b.is_ascii_digit()))
                {
                    return None;
                }
                match p.split('.').count() {
                    3 => self.cartons.contains_key(p).then(|| p.to_string()),
                    2 => self.series(p),
                    1 => self.series(p),
                    _ => None,
                }
            }
            None => self.machine.clone(),
        };
        if let Some(c) = carton {
            if let Some(path) = self.cartons.get(&c).and_then(|t| t.get(tool)) {
                return Some((Some(c), path.clone()));
            }
            if pin.is_some() {
                return None;
            }
        } else if pin.is_some() {
            return None;
        }
        self.sticks.get(tool).map(|p| (None, p.clone()))
    }

    /// Newest installed non-prerelease carton whose version starts with `x.y`
    /// or `x`.
    fn series(&self, prefix: &str) -> Option<String> {
        let want: Vec<u64> = prefix
            .split('.')
            .map(|p| p.parse::<u64>())
            .collect::<Result<_, _>>()
            .ok()?;
        self.cartons
            .keys()
            .filter_map(|c| ver(c).map(|v| (v, c)))
            .filter(|(v, c)| {
                v.pre.is_none()
                    && !self.yanked.contains(c.as_str())
                    && v.major == want[0]
                    && want.get(1).map(|m| v.minor == *m).unwrap_or(true)
            })
            .max_by(|a, b| a.0.cmp(&b.0))
            .map(|(_, c)| c.clone())
    }

    /// Refuse what the shim cannot parse (tabs or newlines in a field) or
    /// what is over 64 KB.
    pub fn check(&self) -> Result<(), JournalError> {
        let bad = |s: &str| s.contains('\t') || s.contains('\n') || s.contains('\r');
        let fields = self
            .machine
            .iter()
            .map(|s| s.as_str().to_string())
            .chain(self.cartons.iter().flat_map(|(c, t)| {
                std::iter::once(c.clone()).chain(
                    t.iter()
                        .flat_map(|(n, p)| [n.clone(), p.display().to_string()]),
                )
            }))
            .chain(
                self.sticks
                    .iter()
                    .flat_map(|(n, p)| [n.clone(), p.display().to_string()]),
            )
            .chain(self.legacy_pin_files.iter().cloned());
        for f in fields {
            if bad(&f) {
                return Err(JournalError::Misuse(format!(
                    "shim-index field {f:?} contains a tab or line break"
                )));
            }
        }
        if self.render().len() > MAX_INDEX_BYTES {
            return Err(JournalError::Misuse(format!(
                "shim-index would exceed {MAX_INDEX_BYTES} bytes"
            )));
        }
        Ok(())
    }

    /// The atomic switch: write `shim-index.new`, log the flip, rename.
    pub fn flip_in(&self, txn: &mut Txn) -> Result<(), JournalError> {
        self.check()?;
        txn.flip(FlipIntent {
            index: self.render(),
        })
    }

    /// Read the live index, `None` when there is none or it is not ours.
    pub fn load(home: &Home) -> Result<Option<ShimIndex>, StateError> {
        let p = home.shim_index_path();
        match std::fs::read_to_string(&p) {
            Ok(t) => Ok(ShimIndex::parse(&t)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(StateError::Io { path: p, source }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ShimIndex {
        let mut i = ShimIndex {
            machine: Some("0.1.0".into()),
            ..ShimIndex::default()
        };
        let p = |s: &str| PathBuf::from(format!("/h/.cigscript/toolchains/{s}"));
        i.cartons.insert(
            "0.1.0".into(),
            BTreeMap::from([
                ("cig".to_string(), p("cig/1.20.0/bin/cig")),
                ("cigo".to_string(), p("cigo/1.17.0/bin/cigo")),
            ]),
        );
        i.cartons.insert(
            "0.1.1".into(),
            BTreeMap::from([("cig".to_string(), p("cig/1.20.1/bin/cig"))]),
        );
        i.cartons.insert(
            "0.2.0".into(),
            BTreeMap::from([("cig".to_string(), p("cig/1.21.0/bin/cig"))]),
        );
        i.cartons.insert(
            "0.3.0-beta.1".into(),
            BTreeMap::from([("cig".to_string(), p("cig/1.22.0/bin/cig"))]),
        );
        i.sticks.insert("cigo".into(), p("cigo/1.16.0/bin/cigo"));
        i
    }

    const GOLDEN: &str = "cigi-index 1
machine\t0.1.0
carton\t0.1.0\tcig\t/h/.cigscript/toolchains/cig/1.20.0/bin/cig
carton\t0.1.0\tcigo\t/h/.cigscript/toolchains/cigo/1.17.0/bin/cigo
carton\t0.1.1\tcig\t/h/.cigscript/toolchains/cig/1.20.1/bin/cig
carton\t0.2.0\tcig\t/h/.cigscript/toolchains/cig/1.21.0/bin/cig
carton\t0.3.0-beta.1\tcig\t/h/.cigscript/toolchains/cig/1.22.0/bin/cig
stick\tcigo\t/h/.cigscript/toolchains/cigo/1.16.0/bin/cigo
carton-minor\t0.1\t0.1.1
carton-minor\t0.2\t0.2.0
carton-major\t0\t0.2.0
legacy-pin-files\t.cig-version\t.cigar-version
";

    #[test]
    fn render_golden() {
        assert_eq!(sample().render(), GOLDEN);
    }

    #[test]
    fn parse_round_trip() {
        let i = sample();
        let back = ShimIndex::parse(&i.render()).unwrap();
        assert_eq!(back, i);
        assert_eq!(ShimIndex::parse("cigi-index 2\n"), None);
    }

    #[test]
    fn pick_follows_section_9() {
        let i = sample();
        let (c, p) = i.pick(Some("0.1.0"), "cigo").unwrap();
        assert_eq!(c.as_deref(), Some("0.1.0"));
        assert!(p.ends_with("cigo/1.17.0/bin/cigo"));
        assert_eq!(
            i.pick(Some("v0.1"), "cig").unwrap().0.as_deref(),
            Some("0.1.1")
        );
        assert_eq!(
            i.pick(Some("0"), "cig").unwrap().0.as_deref(),
            Some("0.2.0")
        );
        assert_eq!(i.pick(Some(">=0.1.0"), "cig"), None);
        assert_eq!(i.pick(Some("0.9.9"), "cig"), None);
        // machine carton, then a stick when the carton lacks the tool
        assert_eq!(i.pick(None, "cig").unwrap().0.as_deref(), Some("0.1.0"));
        let mut j = i.clone();
        j.machine = Some("0.2.0".into());
        let (c, p) = j.pick(None, "cigo").unwrap();
        assert_eq!(c, None);
        assert!(p.ends_with("cigo/1.16.0/bin/cigo"));
        assert_eq!(i.pick(None, "cigcompile"), None);
        // a pin whose carton lacks the tool does not fall back to a stick
        assert_eq!(i.pick(Some("0.2.0"), "cigo"), None);
    }

    #[test]
    fn check_refuses_tabs_and_size() {
        let mut i = sample();
        i.sticks.insert("x".into(), PathBuf::from("/a\tb"));
        assert!(i.check().is_err());
        let mut big = ShimIndex::default();
        let mut tools = BTreeMap::new();
        for n in 0..3000 {
            tools.insert(
                format!("tool{n}"),
                PathBuf::from(format!("/some/long/path/{n}/bin/tool")),
            );
        }
        big.cartons.insert("0.1.0".into(), tools);
        assert!(big.render().len() > MAX_INDEX_BYTES);
        assert!(big.check().is_err());
    }
}
