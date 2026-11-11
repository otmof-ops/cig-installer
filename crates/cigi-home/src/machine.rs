// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! The machine layer (#17): the settings an organization puts on a machine, read only.
//!
//! Three stores hold it, one per kind of system, and all three become the same [`MachineLayer`]:
//!
//! | system | where | locks |
//! |---|---|---|
//! | Linux and the BSDs | `/etc/cigscript/config.toml` | `locked = ["carton", ...]` |
//! | macOS | the managed preferences a configuration profile installs, `/Library/Managed Preferences/io.github.otmof-ops.cigscript.plist` | a `Locked` array of strings |
//! | Windows | `HKLM\SOFTWARE\Policies\otmof-ops\CigScript` | a `Locked` value of type `REG_MULTI_SZ` |
//!
//! Each store is a [`MachineSource`]. The file stores and the registry reader are small types
//! over a file or a [`RegistryRead`], so a test hands them a fixture or a fake and the code that
//! runs is the code that ships. Nothing here ever writes to a store: `ciginstaller` reads the
//! machine layer and an administrator changes it.
//!
//! A store that is there but cannot be trusted or understood is an error that names it
//! ([`StateError::Machine`](crate::StateError)): a lock that cannot be read is not a lock that
//! is absent. On Unix a file source must be a regular file that no one but its owner can write,
//! and the system's own file must belong to root.
//!
//! There is deliberately no environment variable, file or flag that points a release build at
//! another store: such a thing would be a way around every lock. Tests reach the file store
//! through the `test-machine-root` feature, which no release build enables.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::error::StateError;

/// The bundle identifier of the macOS preference domain. The repository had none, so this is the
/// one chosen: the reverse of the project's GitHub home. (The constants are the shim's, which
/// asks the same stores a cheaper question.)
pub const MACOS_DOMAIN: &str = cigi_shim::machine::MACOS_DOMAIN;

/// The directory macOS keeps the preferences a configuration profile installs.
pub const MACOS_MANAGED_DIR: &str = cigi_shim::machine::MACOS_MANAGED_DIR;

/// The file store on Linux and the BSDs.
pub const FILE_PATH: &str = cigi_shim::machine::FILE_PATH;

/// The Windows policy key under `HKEY_LOCAL_MACHINE`, as the registry API takes it.
pub const REGISTRY_SUBKEY: &str = cigi_shim::machine::REGISTRY_SUBKEY;

/// The same key as people write it, and as errors and `locked by` name it.
pub const REGISTRY_LABEL: &str = r"HKLM\SOFTWARE\Policies\otmof-ops\CigScript";

/// The biggest file read as a store: a setting list is a few lines.
const MAX_FILE: u64 = 256 * 1024;

/// A value as a store holds it, before it is checked against the setting it names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Raw {
    Text(String),
    Number(i64),
    Flag(bool),
    List(Vec<String>),
}

/// What the machine layer says: values, and which of them are locked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineLayer {
    source: String,
    values: BTreeMap<String, String>,
    locked: BTreeSet<String>,
}

fn bad(source: &str, why: impl Into<String>) -> StateError {
    StateError::Machine {
        source: source.to_string(),
        why: why.into(),
    }
}

impl MachineLayer {
    /// A layer with nothing in it (a store that exists and says nothing).
    pub fn empty(source: &str) -> MachineLayer {
        MachineLayer {
            source: source.to_string(),
            values: BTreeMap::new(),
            locked: BTreeSet::new(),
        }
    }

    /// Build a layer from a store's entries. `lock_name` is the entry that lists the locks
    /// (`locked`, or `Locked`); `fold` lowercases names first, for a store whose names do not
    /// care about case (the registry). Every setting is checked like a person's `config set`
    /// would check it, and every lock must name a setting: a typo is an error, not a lock that
    /// quietly does nothing.
    pub fn from_entries(
        source: &str,
        entries: Vec<(String, Raw)>,
        lock_name: &str,
        fold: bool,
    ) -> Result<MachineLayer, StateError> {
        let norm = |n: &str| {
            if fold {
                n.to_ascii_lowercase()
            } else {
                n.to_string()
            }
        };
        let lock_key = norm(lock_name);
        let mut layer = MachineLayer::empty(source);
        let mut seen = BTreeSet::new();
        for (name, raw) in entries {
            let name = norm(&name);
            if !seen.insert(name.clone()) {
                return Err(bad(source, format!("`{name}` appears twice")));
            }
            if name == lock_key {
                let Raw::List(keys) = raw else {
                    return Err(bad(
                        source,
                        format!("`{lock_name}` is a list of setting names"),
                    ));
                };
                for k in keys {
                    let k = norm(k.trim());
                    if !Config::is_key(&k) {
                        return Err(bad(
                            source,
                            format!("`{lock_name}` names `{k}`, which is not a setting"),
                        ));
                    }
                    layer.locked.insert(k);
                }
                continue;
            }
            if !Config::is_key(&name) {
                return Err(bad(source, format!("`{name}` is not a setting")));
            }
            let value = match raw {
                Raw::Text(t) => t,
                Raw::Number(n) => n.to_string(),
                Raw::Flag(b) => b.to_string(),
                Raw::List(_) => {
                    return Err(bad(
                        source,
                        format!("`{name}` is a single value, not a list"),
                    ))
                }
            };
            Config::validate(&name, &value).map_err(|e| bad(source, format!("`{name}`: {e}")))?;
            layer.values.insert(name, value);
        }
        Ok(layer)
    }

    /// Where the layer came from, as people read it: a path, a preference domain's file, a key.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// The value this layer sets for `key`, locked or not.
    pub fn value(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    /// Does this layer lock `key`? A key that is locked and has no value is locked at its
    /// built-in default: the person cannot change it, and neither can anything else.
    pub fn is_locked(&self, key: &str) -> bool {
        self.locked.contains(key)
    }

    /// The locked keys, in name order.
    pub fn locked_keys(&self) -> impl Iterator<Item = &str> {
        self.locked.iter().map(String::as_str)
    }
}

/// One store of the machine layer.
pub trait MachineSource {
    /// The store's name for messages, whether or not it exists.
    fn name(&self) -> String;

    /// Read the store. `Ok(None)` when it is not there (no organization set anything up);
    /// `Err` when it is there and cannot be read, trusted or understood.
    fn read(&self) -> Result<Option<MachineLayer>, StateError>;
}

/// A source that holds nothing. What `Config::from_text` uses.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoMachine;

impl MachineSource for NoMachine {
    fn name(&self) -> String {
        "no machine layer".to_string()
    }
    fn read(&self) -> Result<Option<MachineLayer>, StateError> {
        Ok(None)
    }
}

// ----- files ----------------------------------------------------------------------------------

/// Read a store file: absent is `Ok(None)`, and anything unsafe or unreadable names the file.
fn read_store_file(path: &Path, require_root: bool) -> Result<Option<Vec<u8>>, StateError> {
    let label = path.display().to_string();
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(bad(&label, e.to_string())),
    };
    if !meta.is_file() {
        return Err(bad(&label, "it is not a regular file"));
    }
    if meta.len() > MAX_FILE {
        return Err(bad(&label, format!("it is larger than {MAX_FILE} bytes")));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.mode() & 0o022 != 0 {
            return Err(bad(
                &label,
                "anyone but its owner can write it, so it cannot be trusted; remove group and other write access",
            ));
        }
        if require_root && meta.uid() != 0 {
            return Err(bad(
                &label,
                "it is not owned by root, so it cannot be trusted",
            ));
        }
    }
    #[cfg(not(unix))]
    let _ = require_root;
    std::fs::read(path)
        .map(Some)
        .map_err(|e| bad(&label, e.to_string()))
}

/// The TOML store: `key = "value"` lines and `locked = ["key", ...]` at the top level.
#[derive(Debug, Clone)]
pub struct TomlFile {
    path: PathBuf,
    require_root: bool,
}

impl TomlFile {
    /// A file that must belong to root (the system's own).
    pub fn system(path: impl Into<PathBuf>) -> TomlFile {
        TomlFile {
            path: path.into(),
            require_root: true,
        }
    }

    /// A file that must only be unwritable by anyone but its owner (a fixture).
    pub fn owned_by_anyone(path: impl Into<PathBuf>) -> TomlFile {
        TomlFile {
            path: path.into(),
            require_root: false,
        }
    }
}

/// The layer in TOML text. Strings, whole numbers and booleans are settings; `locked` is an
/// array of strings; anything else (a table, a float, a date, a nested array) is refused.
pub fn parse_toml(source: &str, text: &str) -> Result<MachineLayer, StateError> {
    let doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e: toml_edit::TomlError| bad(source, format!("not valid TOML: {e}")))?;
    let mut entries = Vec::new();
    for (k, item) in doc.iter() {
        let toml_edit::Item::Value(v) = item else {
            return Err(bad(
                source,
                format!("`{k}` is a table; settings are plain values"),
            ));
        };
        let raw = match v {
            toml_edit::Value::String(s) => Raw::Text(s.value().clone()),
            toml_edit::Value::Integer(i) => Raw::Number(*i.value()),
            toml_edit::Value::Boolean(b) => Raw::Flag(*b.value()),
            toml_edit::Value::Array(a) => {
                let mut list = Vec::new();
                for x in a.iter() {
                    match x.as_str() {
                        Some(s) => list.push(s.to_string()),
                        None => {
                            return Err(bad(
                                source,
                                format!("`{k}` holds something that is not text"),
                            ))
                        }
                    }
                }
                Raw::List(list)
            }
            _ => {
                return Err(bad(
                    source,
                    format!("`{k}` is not text, a whole number, true or false"),
                ))
            }
        };
        entries.push((k.to_string(), raw));
    }
    MachineLayer::from_entries(source, entries, "locked", false)
}

impl MachineSource for TomlFile {
    fn name(&self) -> String {
        self.path.display().to_string()
    }

    fn read(&self) -> Result<Option<MachineLayer>, StateError> {
        let Some(bytes) = read_store_file(&self.path, self.require_root)? else {
            return Ok(None);
        };
        let label = self.name();
        let text = String::from_utf8(bytes).map_err(|_| bad(&label, "it is not UTF-8 text"))?;
        parse_toml(&label, &text).map(Some)
    }
}

// ----- macOS ----------------------------------------------------------------------------------

/// The macOS store: a property list in the managed preferences.
#[derive(Debug, Clone)]
pub struct PlistFile {
    path: PathBuf,
    require_root: bool,
}

impl PlistFile {
    /// The managed preferences file of [`MACOS_DOMAIN`].
    pub fn system() -> PlistFile {
        PlistFile {
            path: Path::new(MACOS_MANAGED_DIR).join(format!("{MACOS_DOMAIN}.plist")),
            require_root: true,
        }
    }

    /// A plist at any path, owned by anyone (a fixture).
    pub fn at(path: impl Into<PathBuf>) -> PlistFile {
        PlistFile {
            path: path.into(),
            require_root: false,
        }
    }
}

/// A binary property list as XML, through the system's own converter (macOS only: configuration
/// profiles commonly install binary plists, and `plutil` ships with the system).
#[cfg(target_os = "macos")]
fn binary_plist_as_xml(path: &Path) -> Result<String, String> {
    let out = std::process::Command::new("/usr/bin/plutil")
        .args(["-convert", "xml1", "-o", "-", "--"])
        .arg(path)
        .output()
        .map_err(|e| format!("plutil could not run: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "plutil refused it: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    String::from_utf8(out.stdout).map_err(|_| "plutil gave text that is not UTF-8".to_string())
}

#[cfg(not(target_os = "macos"))]
fn binary_plist_as_xml(_path: &Path) -> Result<String, String> {
    Err("it is a binary property list, and only macOS can convert one".to_string())
}

impl MachineSource for PlistFile {
    fn name(&self) -> String {
        self.path.display().to_string()
    }

    fn read(&self) -> Result<Option<MachineLayer>, StateError> {
        let Some(bytes) = read_store_file(&self.path, self.require_root)? else {
            return Ok(None);
        };
        let label = self.name();
        let text = if bytes.starts_with(b"bplist") {
            binary_plist_as_xml(&self.path).map_err(|why| bad(&label, why))?
        } else {
            String::from_utf8(bytes).map_err(|_| bad(&label, "it is not UTF-8 text"))?
        };
        parse_plist(&label, &text).map(Some)
    }
}

/// One tag of the XML plist subset.
#[derive(Debug)]
enum Node {
    Elem(String, Vec<Node>),
    Text(String),
}

fn unescape(s: &str) -> Result<String, String> {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i + 1..];
        let end = tail.find(';').ok_or("an & is not closed")?;
        out.push(match &tail[..end] {
            "amp" => '&',
            "lt" => '<',
            "gt" => '>',
            "quot" => '"',
            "apos" => '\'',
            other => {
                let n = other
                    .strip_prefix("#x")
                    .and_then(|h| u32::from_str_radix(h, 16).ok())
                    .or_else(|| other.strip_prefix('#').and_then(|d| d.parse().ok()));
                n.and_then(char::from_u32)
                    .ok_or_else(|| format!("&{other}; is not a character this reader knows"))?
            }
        });
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Parse the XML subset a property list uses. Recursion is bounded.
fn parse_nodes(s: &str, depth: usize) -> Result<(Vec<Node>, &str, Option<String>), String> {
    if depth > 8 {
        return Err("it nests deeper than a settings file does".to_string());
    }
    let mut nodes = Vec::new();
    let mut rest = s;
    loop {
        let Some(lt) = rest.find('<') else {
            if rest.trim().is_empty() {
                return Ok((nodes, "", None));
            }
            return Err("text outside a tag".to_string());
        };
        let before = &rest[..lt];
        if !before.trim().is_empty() {
            nodes.push(Node::Text(unescape(before.trim())?));
        }
        rest = &rest[lt..];
        if let Some(r) = rest.strip_prefix("<!--") {
            let end = r.find("-->").ok_or("a comment is not closed")?;
            rest = &r[end + 3..];
            continue;
        }
        if rest.starts_with("<?") {
            let end = rest.find("?>").ok_or("a declaration is not closed")?;
            rest = &rest[end + 2..];
            continue;
        }
        if rest.starts_with("<!") {
            // the DOCTYPE; an entity declaration inside it is not read, so nothing expands
            if rest.contains("<!ENTITY") {
                return Err("it declares an entity".to_string());
            }
            let end = rest.find('>').ok_or("a declaration is not closed")?;
            rest = &rest[end + 1..];
            continue;
        }
        let end = rest.find('>').ok_or("a tag is not closed")?;
        let inner = &rest[1..end];
        rest = &rest[end + 1..];
        if let Some(name) = inner.strip_prefix('/') {
            return Ok((nodes, rest, Some(name.trim().to_string())));
        }
        let (name, selfclosed) = match inner.strip_suffix('/') {
            Some(n) => (n.trim(), true),
            None => (inner.trim(), false),
        };
        let name = name.split_whitespace().next().unwrap_or("").to_string();
        if selfclosed {
            nodes.push(Node::Elem(name, Vec::new()));
            continue;
        }
        let (children, after, closed) = parse_nodes(rest, depth + 1)?;
        if closed.as_deref() != Some(name.as_str()) {
            return Err(format!("`<{name}>` is not closed"));
        }
        nodes.push(Node::Elem(name, children));
        rest = after;
    }
}

fn node_text(children: &[Node]) -> String {
    children
        .iter()
        .filter_map(|c| match c {
            Node::Text(t) => Some(t.as_str()),
            Node::Elem(..) => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

fn plist_value(name: &str, node: &Node) -> Result<Raw, String> {
    let Node::Elem(tag, children) = node else {
        return Err(format!("`{name}` has no value"));
    };
    Ok(match tag.as_str() {
        "string" => Raw::Text(node_text(children)),
        "integer" => Raw::Number(
            node_text(children)
                .trim()
                .parse()
                .map_err(|_| format!("`{name}` is not a whole number"))?,
        ),
        "true" => Raw::Flag(true),
        "false" => Raw::Flag(false),
        "array" => Raw::List(
            children
                .iter()
                .map(|c| match c {
                    Node::Elem(t, ch) if t == "string" => Ok(node_text(ch)),
                    _ => Err(format!("`{name}` holds something that is not text")),
                })
                .collect::<Result<_, _>>()?,
        ),
        other => {
            return Err(format!(
                "`{name}` is a <{other}>, not text, a number, true, false or a list"
            ))
        }
    })
}

/// The layer in an XML property list: a top-level `<dict>` of `<string>`, `<integer>`, `<true/>`,
/// `<false/>` and an `<array>` of `<string>` for `Locked`.
pub fn parse_plist(source: &str, text: &str) -> Result<MachineLayer, StateError> {
    let fail = |why: String| {
        bad(
            source,
            format!("not a property list this reader understands: {why}"),
        )
    };
    let (top, _, closed) = parse_nodes(text, 0).map_err(fail)?;
    if closed.is_some() {
        return Err(fail("a closing tag with no opening one".to_string()));
    }
    let plist = top
        .iter()
        .find_map(|n| match n {
            Node::Elem(t, c) if t == "plist" => Some(c),
            _ => None,
        })
        .ok_or_else(|| fail("there is no <plist>".to_string()))?;
    let dict = plist
        .iter()
        .find_map(|n| match n {
            Node::Elem(t, c) if t == "dict" => Some(c),
            _ => None,
        })
        .ok_or_else(|| fail("the top level is not a <dict>".to_string()))?;
    let mut entries = Vec::new();
    let mut it = dict.iter();
    while let Some(k) = it.next() {
        let name = match k {
            Node::Elem(t, c) if t == "key" => node_text(c),
            _ => {
                return Err(fail(
                    "a <dict> entry does not start with a <key>".to_string(),
                ))
            }
        };
        let v = it
            .next()
            .ok_or_else(|| fail(format!("`{name}` has no value")))?;
        entries.push((name.clone(), plist_value(&name, v).map_err(fail)?));
    }
    // a profile's own bookkeeping (`PayloadUUID` and the like) is not a setting
    entries.retain(|(n, _)| !n.starts_with("Payload"));
    MachineLayer::from_entries(source, entries, "Locked", false)
}

// ----- Windows --------------------------------------------------------------------------------

/// The registry key's values, with types kept. The real reader is `cigi_sys::policy`; a test
/// gives a fake.
pub trait RegistryRead {
    /// All values of the policy key; `Ok(None)` when the key does not exist.
    fn values(&self) -> Result<Option<Vec<(String, Raw)>>, String>;
}

/// The Windows store: a registry key read through a [`RegistryRead`].
#[derive(Debug, Clone)]
pub struct RegistrySource<R> {
    label: String,
    reader: R,
}

impl<R: RegistryRead> RegistrySource<R> {
    pub fn new(label: &str, reader: R) -> RegistrySource<R> {
        RegistrySource {
            label: label.to_string(),
            reader,
        }
    }
}

impl<R: RegistryRead> MachineSource for RegistrySource<R> {
    fn name(&self) -> String {
        self.label.clone()
    }

    fn read(&self) -> Result<Option<MachineLayer>, StateError> {
        let Some(entries) = self.reader.values().map_err(|why| bad(&self.label, why))? else {
            return Ok(None);
        };
        MachineLayer::from_entries(&self.label, entries, "Locked", true).map(Some)
    }
}

/// The real registry, `HKLM`, read through `cigi-sys`.
#[cfg(windows)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Hklm;

#[cfg(windows)]
impl RegistryRead for Hklm {
    fn values(&self) -> Result<Option<Vec<(String, Raw)>>, String> {
        use cigi_sys::policy::PolicyValue;
        let Some(vals) =
            cigi_sys::policy::read_values(REGISTRY_SUBKEY).map_err(|e| e.to_string())?
        else {
            return Ok(None);
        };
        let mut out = Vec::new();
        for (name, v) in vals {
            out.push((
                name.clone(),
                match v {
                    PolicyValue::Text(t) => Raw::Text(t),
                    PolicyValue::Number(n) => Raw::Number(i64::from(n)),
                    PolicyValue::List(l) => Raw::List(l),
                    PolicyValue::Other(t) => {
                        return Err(format!(
                            "`{name}` has registry type {t}, which is not text, a number or a list"
                        ))
                    }
                },
            ));
        }
        Ok(Some(out))
    }
}

// ----- the system's own store -----------------------------------------------------------------

/// The environment variable the `test-machine-root` feature reads: a directory whose
/// `config.toml` stands in for the system's store. Release builds do not have the feature, and
/// so cannot be pointed anywhere.
#[cfg(feature = "test-machine-root")]
pub const TEST_ROOT_SEAM: &str = "CIGI_TEST_MACHINE_ROOT";

/// The store of this system: the file on Linux and the BSDs, the managed preferences on macOS,
/// the registry on Windows.
pub fn system() -> Box<dyn MachineSource> {
    #[cfg(feature = "test-machine-root")]
    if let Some(root) = std::env::var_os(TEST_ROOT_SEAM).filter(|v| !v.is_empty()) {
        return Box::new(TomlFile::owned_by_anyone(
            Path::new(&root).join("config.toml"),
        ));
    }
    system_store()
}

#[cfg(windows)]
fn system_store() -> Box<dyn MachineSource> {
    Box::new(RegistrySource::new(REGISTRY_LABEL, Hklm))
}

#[cfg(target_os = "macos")]
fn system_store() -> Box<dyn MachineSource> {
    Box::new(PlistFile::system())
}

#[cfg(not(any(windows, target_os = "macos")))]
fn system_store() -> Box<dyn MachineSource> {
    Box::new(TomlFile::system(FILE_PATH))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn txt(s: &str) -> Raw {
        Raw::Text(s.to_string())
    }

    fn list(v: &[&str]) -> Raw {
        Raw::List(v.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn a_toml_store_has_values_and_locks() {
        let l = parse_toml(
            "/etc/cigscript/config.toml",
            "# set by IT\nauto_update = \"off\"\ncarton = \"1.1\"\nkeep_days = 30\nlocked = [\"carton\", \"auto_update\"]\n",
        )
        .unwrap();
        assert_eq!(l.value("carton"), Some("1.1"));
        assert_eq!(l.value("keep_days"), Some("30"));
        assert!(l.is_locked("carton") && l.is_locked("auto_update") && !l.is_locked("keep_days"));
        assert_eq!(l.source(), "/etc/cigscript/config.toml");
    }

    #[test]
    fn a_malformed_toml_store_is_an_error_that_names_it() {
        for (text, needle) in [
            ("auto_update = \n", "not valid TOML"),
            ("bogus = \"x\"\n", "`bogus` is not a setting"),
            ("channel = \"sideways\"\n", "`channel`"),
            ("locked = [\"nope\"]\n", "`nope`, which is not a setting"),
            ("locked = \"carton\"\n", "list of setting names"),
            ("[tool]\nx = 1\n", "is a table"),
            ("carton = 1.5\n", "not text, a whole number"),
            ("carton = \"not a pin\"\n", "`carton`"),
            ("mirror = \"http://example.com/\"\n", "`mirror`"),
        ] {
            let e = parse_toml("/etc/cigscript/config.toml", text).unwrap_err();
            assert_eq!(e.code(), "E1830", "{text}");
            let m = e.to_string();
            assert!(
                m.contains("/etc/cigscript/config.toml") && m.contains(needle),
                "{m}"
            );
        }
    }

    #[test]
    fn a_missing_store_is_nothing_and_a_loose_one_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("config.toml");
        let src = TomlFile::owned_by_anyone(&p);
        assert!(src.read().unwrap().is_none());
        std::fs::write(&p, "carton = \"1.1\"\nlocked = [\"carton\"]\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o666)).unwrap();
            let e = src.read().unwrap_err().to_string();
            assert!(e.contains("config.toml") && e.contains("write"), "{e}");
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
            // a test user is not root: the system's own file rule refuses it
            let e = TomlFile::system(&p).read().unwrap_err().to_string();
            assert!(e.contains("not owned by root"), "{e}");
        }
        assert!(src.read().unwrap().unwrap().is_locked("carton"));
        std::fs::create_dir(tmp.path().join("dir")).unwrap();
        let e = TomlFile::owned_by_anyone(tmp.path().join("dir"))
            .read()
            .unwrap_err();
        assert!(e.to_string().contains("regular file"), "{e}");
    }

    const PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>PayloadUUID</key><string>5F3C-ignored</string>
  <key>carton</key><string>1.1</string>
  <key>reports</key><string>never</string>
  <key>keep_days</key><integer>45</integer>
  <key>allow_single_source</key><false/>
  <key>mirror</key><string>https://mirror.example.com/a&amp;b</string>
  <key>Locked</key>
  <array><string>carton</string><string>reports</string></array>
</dict>
</plist>"#;

    #[test]
    fn a_plist_store_reads_through_the_same_layer() {
        let l = parse_plist("managed.plist", PLIST).unwrap();
        assert_eq!(l.value("carton"), Some("1.1"));
        assert_eq!(l.value("keep_days"), Some("45"));
        assert_eq!(l.value("allow_single_source"), Some("false"));
        assert_eq!(l.value("mirror"), Some("https://mirror.example.com/a&b"));
        assert!(l.is_locked("carton") && l.is_locked("reports") && !l.is_locked("mirror"));
        assert_eq!(l.locked_keys().collect::<Vec<_>>(), ["carton", "reports"]);
    }

    #[test]
    fn a_plist_file_is_read_from_disk_and_a_bad_one_names_itself() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("io.github.otmof-ops.cigscript.plist");
        put(&p, PLIST.as_bytes());
        let l = PlistFile::at(&p).read().unwrap().unwrap();
        assert!(l.is_locked("reports"));
        for (text, needle) in [
            (
                "<plist><dict><key>carton</key></dict></plist>",
                "has no value",
            ),
            (
                "<plist><dict><key>carton</key><real>1</real></dict></plist>",
                "<real>",
            ),
            (
                "<plist><dict><key>Locked</key><string>carton</string></dict></plist>",
                "list of setting names",
            ),
            (
                "<plist><dict><key>nope</key><string>x</string></dict></plist>",
                "not a setting",
            ),
            ("<plist><dict></plist>", "not closed"),
            ("<plist><array/></plist>", "not a <dict>"),
            (
                "<!DOCTYPE x [<!ENTITY a \"b\">]><plist><dict/></plist>",
                "entity",
            ),
            ("nothing", "text outside a tag"),
            ("<dict/>", "no <plist>"),
        ] {
            put(&p, text.as_bytes());
            let e = PlistFile::at(&p).read().unwrap_err();
            assert_eq!(e.code(), "E1830");
            let m = e.to_string();
            assert!(m.contains(".plist") && m.contains(needle), "{text}: {m}");
        }
        put(&p, b"bplist00\x00");
        #[cfg(not(target_os = "macos"))]
        assert!(PlistFile::at(&p)
            .read()
            .unwrap_err()
            .to_string()
            .contains("binary property list"));
    }

    /// A store file as an administrator leaves it: nobody else can write it.
    fn put(p: &Path, text: &[u8]) {
        std::fs::write(p, text).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
    }

    struct FakeRegistry(Result<Option<Vec<(String, Raw)>>, String>);

    impl RegistryRead for FakeRegistry {
        fn values(&self) -> Result<Option<Vec<(String, Raw)>>, String> {
            self.0.clone()
        }
    }

    #[test]
    fn a_registry_store_reads_through_the_same_layer() {
        // REG_SZ, REG_DWORD and REG_MULTI_SZ, with the case Windows lets a value name keep
        let vals = vec![
            ("Carton".to_string(), txt("1.1")),
            ("AUTO_UPDATE".to_string(), txt("security")),
            ("Keep_Days".to_string(), Raw::Number(30)),
            ("Locked".to_string(), list(&["carton", "Auto_Update"])),
        ];
        let s = RegistrySource::new(REGISTRY_LABEL, FakeRegistry(Ok(Some(vals))));
        let l = s.read().unwrap().unwrap();
        assert_eq!(l.value("carton"), Some("1.1"));
        assert_eq!(l.value("auto_update"), Some("security"));
        assert_eq!(l.value("keep_days"), Some("30"));
        assert_eq!(l.source(), REGISTRY_LABEL);
        assert!(l.is_locked("carton"));
        // the names in the lock list fold like every other name here
        assert!(l.is_locked("auto_update"));
    }

    #[test]
    fn a_registry_that_is_absent_is_nothing_and_a_broken_one_names_the_key() {
        let none = RegistrySource::new(REGISTRY_LABEL, FakeRegistry(Ok(None)));
        assert!(none.read().unwrap().is_none());
        let broken = RegistrySource::new(REGISTRY_LABEL, FakeRegistry(Err("access denied".into())));
        let e = broken.read().unwrap_err().to_string();
        assert!(
            e.contains(REGISTRY_LABEL) && e.contains("access denied"),
            "{e}"
        );
        let bad_value = RegistrySource::new(
            REGISTRY_LABEL,
            FakeRegistry(Ok(Some(vec![("Locked".to_string(), txt("carton"))]))),
        );
        assert!(bad_value
            .read()
            .unwrap_err()
            .to_string()
            .contains("list of setting names"));
        let dup = RegistrySource::new(
            REGISTRY_LABEL,
            FakeRegistry(Ok(Some(vec![
                ("Channel".to_string(), txt("beta")),
                ("channel".to_string(), txt("stable")),
            ]))),
        );
        assert!(dup
            .read()
            .unwrap_err()
            .to_string()
            .contains("appears twice"));
    }
}
