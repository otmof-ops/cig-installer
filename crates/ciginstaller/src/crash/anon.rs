// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! The text side of the allowlist (#28 part 1, section 2): what a report may say about a
//! machine, and how.
//!
//! - A path that is the person's becomes its shape (`~/d1/d2/f3.txt`, `/d1/f2`): the depth, a
//!   capital, a leading dot and an extension survive, no name does.
//! - A path that is the family's own stays as it is: `ashtray/.flush.lock` is never turned into
//!   `/d1/d2/d3/.f1.lock` in a title (cig-script#127). Own paths are the installer's list below,
//!   whatever a product registers (`register_own_paths`), anything under a family folder
//!   (`.cigscript`) and anything under this run's own home.
//! - The person's user name and this machine's host name never travel, wherever they are written.
//! - A token-shaped word, a URL, an address and an e-mail address are replaced by what they are.
//! - Ordinary words are never touched.
//!
//! Nothing here reads a file's contents or an environment variable's value into a report: the
//! home directory, user and host names are read only to be removed.

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use serde_json::Value;
use sha2::{Digest, Sha256};

/// Paths the family's products own. Matched component by component against the end of a path
/// (a longer path under one of them is kept whole). One place, tested by
/// `own_words_survive_anonymising`; products add theirs with [`register_own_paths`].
const OWN_PATHS: &[&str] = &[
    "ashtray/.flush.lock",
    "installer/state.json",
    "installer/config",
    "installer/shim-index",
    "installer/lock",
    "installer/lock.pid",
    "installer/txn",
    "installer/cache",
    "installer.lock",
    "tin/statcache",
    "orchestrate.cig",
    "state/versions.json",
    ".cig-carton",
    ".cig-version",
    ".cigo-version",
    ".cigar-version",
    "cig-tasks.json",
    "cigar.toml",
];

/// A folder the family owns whole: from here down, every name is ours.
const OWN_ROOTS: &[&str] = &[".cigscript", ".cig-orchestrator", ".cigi"];

/// Words an installer command line may carry as they are, beside the subcommands and flags the
/// parser registers.
const COMMAND_WORDS: &[&str] = &[
    "ciginstaller",
    "cigi",
    "cig",
    "cigo",
    "cigcompile",
    "cigar",
    "cigar-lsp",
    "stable",
    "beta",
    "nightly",
    "always",
    "never",
    "ask",
    "on",
    "off",
    "true",
    "false",
    "standard",
    "carton",
    "pack",
    "tool",
];

const SECRET_FLAGS: &[&str] = &["token", "key", "secret", "password", "passwd", "auth"];

static REGISTERED: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
static KNOWN: OnceLock<HashSet<String>> = OnceLock::new();

/// A product registers the paths that are its own, so the anonymiser keeps them readable
/// (`crash submit` takes them from the report's `own_paths`). Only a relative path of two parts
/// or more, or a file with an extension, can be registered: a product cannot register a
/// person's folder.
pub fn register_own_paths<I: IntoIterator<Item = String>>(paths: I) {
    let m = REGISTERED.get_or_init(|| Mutex::new(Vec::new()));
    if let Ok(mut g) = m.lock() {
        for p in paths.into_iter().take(64) {
            let p = slashes(&p);
            let parts: Vec<&str> = p.split('/').filter(|c| !c.is_empty()).collect();
            let absolute = p.starts_with('/') || p.starts_with('~') || is_drive(&p);
            if !absolute
                && p.len() <= 200
                && !parts.contains(&"..")
                && (parts.len() >= 2 || parts.first().is_some_and(|f| f.contains('.')))
            {
                g.push(p);
            }
        }
    }
}

/// The words the command line may say (the parser's subcommands and flags).
pub fn know_words<I: IntoIterator<Item = String>>(words: I) {
    let _ = KNOWN.set(words.into_iter().collect());
}

fn known(word: &str) -> bool {
    COMMAND_WORDS.contains(&word)
        || cigi_home::config::KEYS.iter().any(|k| k.key == word)
        || KNOWN.get().is_some_and(|s| s.contains(word))
}

/// What differs between machines, read once per anonymisation.
struct Env {
    /// Every spelling of the user's home directory, `/`-separated, longest first.
    homes: Vec<String>,
    /// The folder this run's `CIGSCRIPT_HOME` names, `/`-separated.
    cig_home: Option<String>,
    /// The user's and the machine's names, each with what it is.
    names: Vec<(String, &'static str)>,
    own: Vec<Vec<String>>,
}

fn slashes(s: &str) -> String {
    s.replace('\\', "/")
}

impl Env {
    fn read(cig_home: Option<&Path>) -> Env {
        let mut homes = Vec::new();
        let mut names: Vec<(String, &'static str)> = Vec::new();
        for key in ["HOME", "USERPROFILE"] {
            if let Some(h) = std::env::var_os(key).filter(|h| !h.is_empty()) {
                let raw = slashes(&h.to_string_lossy());
                if let Ok(c) = std::fs::canonicalize(&raw) {
                    let c = slashes(&c.to_string_lossy());
                    if c != raw {
                        homes.push(c);
                    }
                }
                if let Some(base) = raw.trim_end_matches('/').rsplit('/').next() {
                    names.push((base.to_string(), "<user>"));
                }
                homes.push(raw);
            }
        }
        for (key, what) in [
            ("USER", "<user>"),
            ("USERNAME", "<user>"),
            ("LOGNAME", "<user>"),
            ("HOSTNAME", "<host>"),
            ("COMPUTERNAME", "<host>"),
        ] {
            if let Ok(v) = std::env::var(key) {
                names.push((v, what));
            }
        }
        for f in ["/etc/hostname", "/proc/sys/kernel/hostname"] {
            if let Ok(v) = std::fs::read_to_string(f) {
                names.push((v.trim().to_string(), "<host>"));
            }
        }
        homes.retain(|h| h.len() > 1);
        homes.sort_by_key(|h| std::cmp::Reverse(h.len()));
        homes.dedup();
        names.retain(|(n, _)| {
            n.chars().count() >= 3 && !matches!(n.as_str(), "root" | "user" | "home")
        });
        names.sort_by_key(|(n, _)| std::cmp::Reverse(n.len()));
        names.dedup();
        let mut own: Vec<Vec<String>> = OWN_PATHS
            .iter()
            .map(|p| p.split('/').map(str::to_string).collect())
            .collect();
        if let Some(m) = REGISTERED.get().and_then(|m| m.lock().ok()) {
            own.extend(m.iter().map(|p| {
                slashes(p)
                    .split('/')
                    .filter(|c| !c.is_empty())
                    .map(str::to_string)
                    .collect()
            }));
        }
        Env {
            homes,
            cig_home: cig_home.map(|p| {
                slashes(&p.to_string_lossy())
                    .trim_end_matches('/')
                    .to_string()
            }),
            names,
            own,
        }
    }
}

fn looks_like_token(s: &str) -> bool {
    const PREFIXES: [&str; 7] = ["ghp_", "gho_", "ghs_", "github_pat_", "sk-", "xox", "AKIA"];
    if PREFIXES.iter().any(|p| s.starts_with(p)) {
        return true;
    }
    s.len() >= 32
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn looks_like_ipv4(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() == 4
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.len() <= 3
                && p.bytes().all(|b| b.is_ascii_digit())
                && p.parse::<u16>().is_ok_and(|n| n <= 255)
        })
}

fn is_drive(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b.len() == 2 || b[2] == b'/')
}

/// A word that names a place on disk. `and/or` is prose, `~/work/notes.txt` and `C:\Users\x` are not.
/// `absolute_only`: a relative `src/x.rs` is left alone (a stack frame's own source).
fn pathish_in(core: &str, absolute_only: bool) -> bool {
    if core.starts_with('~') || core.starts_with('/') || is_drive(&slashes(core)) {
        return true;
    }
    if core.contains('\\') {
        return true;
    }
    if absolute_only || !core.contains('/') {
        return false;
    }
    let one_slash = core.matches('/').count() == 1;
    let prose = one_slash && core.chars().all(|c| c.is_ascii_alphabetic() || c == '/');
    !prose
}

/// The placeholder for one component, by position: `d2`, `f3.txt`, `.d1`, `D4`.
fn is_placeholder(name: &str) -> bool {
    let body = name.trim_start_matches('.');
    let (stem, ext) = body.split_once('.').map_or((body, ""), |(a, b)| (a, b));
    let mut it = stem.chars();
    let letter = it
        .next()
        .is_some_and(|c| matches!(c, 'd' | 'D' | 'f' | 'F'));
    let rest: String = it.collect();
    let digits = rest.strip_prefix('u').unwrap_or(&rest);
    letter
        && !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit())
        && ext.bytes().all(|b| b.is_ascii_alphanumeric())
}

fn placeholder(name: &str, index: usize, last: bool) -> String {
    if is_placeholder(name) {
        return name.to_string();
    }
    let lead_dot = name.starts_with('.');
    let body = name.trim_start_matches('.');
    let upper = body.chars().next().is_some_and(char::is_uppercase);
    let ext = if last {
        body.rsplit_once('.').map(|(_, e)| e).filter(|e| {
            !e.is_empty() && e.len() <= 8 && e.bytes().all(|b| b.is_ascii_alphanumeric())
        })
    } else {
        None
    };
    let letter = match (last, upper) {
        (false, false) => 'd',
        (false, true) => 'D',
        (true, false) => 'f',
        (true, true) => 'F',
    };
    let wide = if body.is_ascii() { "" } else { "u" };
    let mut s = String::new();
    if lead_dot {
        s.push('.');
    }
    s.push(letter);
    s.push_str(wide);
    s.push_str(&(index + 1).to_string());
    if let Some(e) = ext {
        s.push('.');
        s.push_str(e);
    }
    s
}

/// The shape of a path, own names kept. `core` carries no quotes; a `:12:5` trailer is kept.
fn shape_path(core: &str, env: &Env, last_is_dir: bool) -> String {
    let mut path = slashes(core);
    let mut trailer = String::new();
    while let Some((head, tail)) = path.rsplit_once(':') {
        let drive = head.len() == 1 && head.as_bytes()[0].is_ascii_alphabetic();
        if tail.is_empty() || !tail.bytes().all(|b| b.is_ascii_digit()) || head.is_empty() || drive
        {
            break;
        }
        trailer = format!(":{tail}{trailer}");
        path = head.to_string();
    }
    // base: what the path starts from
    let mut base = String::new();
    let mut rest = path.as_str();
    let mut whole_kept_from: Option<usize> = None;
    if let Some(ch) = env.cig_home.as_deref().filter(|c| !c.is_empty()) {
        if let Some(r) = path
            .strip_prefix(ch)
            .filter(|r| r.is_empty() || r.starts_with('/'))
        {
            base = "$CIGSCRIPT_HOME".into();
            rest = r;
            whole_kept_from = Some(0);
        }
    }
    if base.is_empty() {
        // already shaped by an earlier pass
        if let Some(r) = path
            .strip_prefix("$CIGSCRIPT_HOME")
            .filter(|r| r.is_empty() || r.starts_with('/'))
        {
            base = "$CIGSCRIPT_HOME".into();
            rest = r;
            whole_kept_from = Some(0);
        }
    }
    if base.is_empty() {
        if let Some(r) = rest.strip_prefix('~') {
            base = "~".into();
            rest = r;
        } else if let Some(r) = env.homes.iter().find_map(|h| {
            rest.strip_prefix(h.as_str())
                .filter(|r| r.is_empty() || r.starts_with('/'))
        }) {
            base = "~".into();
            rest = r;
        } else if is_drive(rest) {
            base = "/".into();
            rest = &rest[2..];
        } else if rest.starts_with('/') {
            base = "/".into();
        }
    }
    let mut comps: Vec<&str> = rest
        .split('/')
        .filter(|c| !c.is_empty() && *c != ".")
        .collect();
    // a family folder keeps what is below it
    if whole_kept_from.is_none() {
        whole_kept_from = comps.iter().position(|c| OWN_ROOTS.contains(c));
    }
    if whole_kept_from.is_none() {
        for i in 0..comps.len() {
            if env
                .own
                .iter()
                .any(|o| comps[i..].len() >= o.len() && comps[i..i + o.len()] == o[..])
            {
                whole_kept_from = Some(i);
                break;
            }
        }
    }
    let keep_from = whole_kept_from.unwrap_or(comps.len());
    let n = comps.len();
    let mut shaped: Vec<String> = Vec::with_capacity(n);
    for (i, c) in comps.drain(..).enumerate() {
        if i >= keep_from || c == ".." {
            shaped.push(c.to_string());
        } else {
            shaped.push(placeholder(c, i, i + 1 == n && !last_is_dir));
        }
    }
    let body = shaped.join("/");
    let joined = match base.as_str() {
        "~" | "$CIGSCRIPT_HOME" if body.is_empty() => base.clone(),
        "~" | "$CIGSCRIPT_HOME" => format!("{base}/{body}"),
        "/" if body.is_empty() => "/".to_string(),
        "/" => format!("/{body}"),
        _ => body,
    };
    format!("{joined}{trailer}")
}

/// Split a word into the punctuation that wraps it and the word itself.
fn split_word(w: &str) -> (&str, &str, &str) {
    let is_wrap = |c: char| {
        matches!(
            c,
            '"' | '\'' | '`' | '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>' | ',' | ';' | '='
        )
    };
    let start = w.find(|c: char| !is_wrap(c)).unwrap_or(w.len());
    let end = w
        .rfind(|c: char| !(is_wrap(c) || c == '.'))
        .map(|i| i + w[i..].chars().next().map_or(1, char::len_utf8))
        .unwrap_or(start)
        .max(start);
    (&w[..start], &w[start..end], &w[end..])
}

fn word(w: &str, env: &Env, absolute_only: bool) -> String {
    let (lead, core, trail) = split_word(w);
    if core.is_empty() {
        return w.to_string();
    }
    // `key=value`: a secret's value is gone, any other value is read as a word of its own
    if let Some((key, value)) = core.split_once('=') {
        if !key.is_empty() && !key.contains('/') && !key.contains('\\') {
            let secret = SECRET_FLAGS
                .iter()
                .any(|f| key.to_ascii_lowercase().contains(f));
            let shown = if secret {
                "<redacted>".to_string()
            } else {
                word(value, env, absolute_only)
            };
            return format!("{lead}{key}={shown}{trail}");
        }
    }
    if core.contains("://") {
        return format!("{lead}<url>{trail}");
    }
    if looks_like_token(core) {
        return format!("{lead}<redacted>{trail}");
    }
    if looks_like_ipv4(core) {
        return format!("{lead}<ip>{trail}");
    }
    if core.contains('@')
        && core.contains('.')
        && !core.starts_with('@')
        && !pathish_in(core, absolute_only)
    {
        return format!("{lead}<email>{trail}");
    }
    if pathish_in(core, absolute_only) {
        return format!("{lead}{}{trail}", shape_path(core, env, false));
    }
    w.to_string()
}

fn scrub_names(s: String, env: &Env) -> String {
    let mut out = s;
    for (n, what) in &env.names {
        if out.contains(n.as_str()) {
            out = out.replace(n.as_str(), what);
        }
    }
    out
}

/// Free text as a report may carry it. `cig_home` is this run's own home, when known.
pub fn anonymise_text_in(text: &str, cig_home: Option<&Path>) -> String {
    rewrite(text, cig_home, false)
}

/// The light pass over text that is already a report's (a stack frame, a location, a command
/// word): an absolute path is shaped, names and tokens are removed, a relative `src/x.rs` stays.
pub fn scrub_text(text: &str, cig_home: Option<&Path>) -> String {
    rewrite(text, cig_home, true)
}

fn rewrite(text: &str, cig_home: Option<&Path>, absolute_only: bool) -> String {
    let env = Env::read(cig_home);
    let mut out = String::with_capacity(text.len());
    let mut run = String::new();
    let mut in_space = false;
    let flush = |run: &mut String, out: &mut String, space: bool| {
        if run.is_empty() {
            return;
        }
        if space {
            out.push_str(run);
        } else {
            out.push_str(&word(run, &env, absolute_only));
        }
        run.clear();
    };
    for c in text.chars() {
        if c.is_whitespace() != in_space {
            flush(&mut run, &mut out, in_space);
            in_space = c.is_whitespace();
        }
        run.push(c);
    }
    flush(&mut run, &mut out, in_space);
    scrub_names(out, &env)
}

/// Every string in a JSON value, anonymised; keys stay.
pub fn anonymise_value(v: &mut Value, cig_home: Option<&Path>) {
    match v {
        Value::String(s) => *s = anonymise_text_in(s, cig_home),
        Value::Array(a) => a.iter_mut().for_each(|x| anonymise_value(x, cig_home)),
        Value::Object(o) => o.values_mut().for_each(|x| anonymise_value(x, cig_home)),
        _ => {}
    }
}

// ----- a person's own words: marked, never rewritten silently -------------------------------

/// A span of a person's own text that the anonymiser would change: what it is, as typed, and
/// what it would become. Asking the person is the caller's job; nothing here rewrites on its own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mark {
    /// `path`, `address` (an IP or a URL), `token`, `email` or `name` (this user's or host's).
    pub kind: &'static str,
    pub text: String,
    pub replace: String,
}

impl Mark {
    /// A secret-shaped span is never kept: sending it would hand a credential over.
    pub fn keepable(&self) -> bool {
        self.kind != "token"
    }
}

/// Every span of `text` shaped like a path, an address, a token or an e-mail address, and this
/// user's and host's names, each once, in the order they first appear.
pub fn marks(text: &str, cig_home: Option<&Path>) -> Vec<Mark> {
    let env = Env::read(cig_home);
    let mut out: Vec<Mark> = Vec::new();
    for w in text.split_whitespace() {
        let (_, core, _) = split_word(w);
        if core.is_empty() || out.iter().any(|m| m.text == core) {
            continue;
        }
        let shown = word(core, &env, false);
        if shown == core {
            continue;
        }
        let kind = if core.contains("://") || shown.contains("<url>") || shown.contains("<ip>") {
            "address"
        } else if shown.contains("<redacted>") {
            "token"
        } else if shown.contains("<email>") {
            "email"
        } else {
            "path"
        };
        out.push(Mark {
            kind,
            text: core.to_string(),
            replace: shown,
        });
    }
    let mut rest = text.to_string();
    for m in &out {
        rest = rest.replace(&m.text, " ");
    }
    for (n, what) in &env.names {
        if rest.contains(n.as_str()) && !out.iter().any(|m| m.text == *n) {
            out.push(Mark {
                kind: "name",
                text: n.clone(),
                replace: (*what).to_string(),
            });
        }
    }
    out
}

/// `text` with every mark `keep` says no to replaced as the anonymiser would; the others stay
/// exactly as typed.
pub fn apply_marks(text: &str, marks: &[Mark], keep: &dyn Fn(&Mark) -> bool) -> String {
    let mut order: Vec<&Mark> = marks.iter().filter(|m| !keep(m)).collect();
    order.sort_by_key(|m| std::cmp::Reverse(m.text.len()));
    let mut out = text.to_string();
    for m in order {
        out = out.replace(&m.text, &m.replace);
    }
    out
}

fn keeping(s: &str, home: Option<&Path>, kept: &[String]) -> String {
    let mut order: Vec<&String> = kept.iter().filter(|k| !k.is_empty()).collect();
    order.sort_by_key(|k| std::cmp::Reverse(k.len()));
    let mut t = s.to_string();
    for (i, k) in order.iter().enumerate() {
        t = t.replace(k.as_str(), &format!("\u{1}{i}\u{1}"));
    }
    let mut out = anonymise_text_in(&t, home);
    for (i, k) in order.iter().enumerate() {
        out = out.replace(&format!("\u{1}{i}\u{1}"), k);
    }
    out
}

/// A report's `details`, anonymised: every string, as always. The one exception is a bug report
/// (`details.kind` is `ciginstaller-bug`): the spans the person was shown and chose to keep
/// (`kept`, which stays on this machine) pass through `details.answers` as typed, and nowhere else.
pub fn anonymise_details(d: &mut Value, cig_home: Option<&Path>, kept: &[String]) {
    let is_bug = d["kind"] == "ciginstaller-bug";
    if kept.is_empty() || !is_bug {
        return anonymise_value(d, cig_home);
    }
    let answers = d.get_mut("answers").map(Value::take);
    anonymise_value(d, cig_home);
    if let (Some(mut a), Some(o)) = (answers, d.as_object_mut()) {
        keep_in(&mut a, cig_home, kept);
        o.insert("answers".into(), a);
    }
}

fn keep_in(v: &mut Value, home: Option<&Path>, kept: &[String]) {
    match v {
        Value::String(s) => *s = keeping(s, home, kept),
        Value::Array(a) => a.iter_mut().for_each(|x| keep_in(x, home, kept)),
        Value::Object(o) => o.values_mut().for_each(|x| keep_in(x, home, kept)),
        _ => {}
    }
}

/// The shape of a directory, as a report's `cwd`.
pub fn shape(p: &Path, cig_home: Option<&Path>) -> String {
    let env = Env::read(cig_home);
    scrub_names(shape_path(&p.to_string_lossy(), &env, true), &env)
}

// ----- command lines --------------------------------------------------------------------

fn is_version(s: &str) -> bool {
    let s = s.strip_prefix('v').unwrap_or(s);
    let core = s.split(['-', '+']).next().unwrap_or("");
    core.contains('.')
        && core
            .split('.')
            .all(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+'))
}

fn is_code(s: &str) -> bool {
    s.len() == 5 && s.starts_with('E') && s[1..].bytes().all(|b| b.is_ascii_digit())
}

/// One argument as a report carries it: a word the installer knows, a version, an error code or a
/// number as it is; a secret-shaped value `<redacted>`; a URL `<url>`; everything else as what it
/// is, `<path>`, `<flag>` or `<arg>`.
pub fn redact_arg(arg: &str) -> String {
    if arg.is_empty() {
        return String::new();
    }
    let lower = arg.to_ascii_lowercase();
    if let Some((flag, _)) = arg.split_once('=') {
        if flag.starts_with('-') {
            let secret = SECRET_FLAGS
                .iter()
                .any(|s| flag.to_ascii_lowercase().contains(s));
            let shown = if known(flag.trim_start_matches('-')) || secret {
                flag
            } else {
                "<flag>"
            };
            return format!("{shown}={}", if secret { "<redacted>" } else { "<value>" });
        }
    }
    if SECRET_FLAGS.iter().any(|s| lower.contains(s)) && !arg.starts_with('-') {
        return "<redacted>".into();
    }
    if looks_like_token(arg) {
        return "<redacted>".into();
    }
    if arg.contains("://") {
        return "<url>".into();
    }
    let bare = arg.trim_start_matches('-');
    if known(arg)
        || (arg.starts_with('-') && known(bare))
        || arg.bytes().all(|b| b.is_ascii_digit())
        || is_version(arg)
        || is_code(arg)
    {
        return arg.to_string();
    }
    if arg.starts_with('-') {
        return "<flag>".into();
    }
    if arg.contains('/') || arg.contains('\\') {
        return "<path>".into();
    }
    "<arg>".into()
}

/// A command line as a report carries it: the program's name, then every argument redacted.
pub fn redact_command<I: IntoIterator<Item = String>>(args: I) -> Vec<String> {
    args.into_iter()
        .enumerate()
        .map(|(i, a)| {
            if i == 0 {
                let base = a.rsplit(['/', '\\']).next().unwrap_or(&a);
                let base = base.strip_suffix(".exe").unwrap_or(base);
                if matches!(base, "ciginstaller" | "cigi") {
                    base.to_string()
                } else {
                    "<program>".to_string()
                }
            } else {
                redact_arg(&a)
            }
        })
        .collect()
}

// ----- the fingerprint ------------------------------------------------------------------

/// A run of digits is one `#` and a path is `<path>`: the same fault at another line, in a folder named
/// otherwise, on another machine, reads the same.
fn steady(s: &str) -> String {
    s.split_whitespace()
        .map(|w| {
            if w.contains('/') || w.contains('\\') || w.starts_with('~') {
                "<path>".to_string()
            } else {
                let mut out = String::with_capacity(w.len());
                for c in w.chars() {
                    if !c.is_ascii_digit() {
                        out.push(c);
                    } else if !out.ends_with('#') {
                        out.push('#');
                    }
                }
                out
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The family's fingerprint: cig's (kind, location, message with numbers removed) plus the
/// product and its major version (#28 section 3). Eight hex characters. The inputs are the
/// anonymised location and message, and nothing about the machine, so no path, user name or host
/// can reach it.
pub fn fingerprint(
    origin: &str,
    version: &str,
    kind: &str,
    location: &str,
    message: &str,
) -> String {
    let major = version.split('.').next().unwrap_or("0");
    let mut h = Sha256::new();
    for part in [origin, major, kind, &steady(location), &steady(message)] {
        h.update(part.as_bytes());
        h.update([0u8]);
    }
    hex::encode(h.finalize())[..8].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> Env {
        Env {
            homes: vec!["/home/jay".into()],
            cig_home: Some("/home/jay/.cigscript".into()),
            names: vec![("jay".into(), "<user>"), ("workhorsey".into(), "<host>")],
            own: OWN_PATHS
                .iter()
                .map(|p| p.split('/').map(str::to_string).collect())
                .collect(),
        }
    }

    #[test]
    fn a_persons_path_is_its_shape() {
        assert_eq!(
            shape_path("/home/jay/work/secret/Notes.txt", &env(), false),
            "~/d1/d2/F3.txt"
        );
        assert_eq!(shape_path("/srv/data/.hidden", &env(), false), "/d1/d2/.f3");
        assert_eq!(
            shape_path("C:\\Users\\x\\a.cig", &env(), false),
            "/D1/d2/f3.cig"
        );
        assert_eq!(
            shape_path("/home/jay/work/a.rs:12:5", &env(), false),
            "~/d1/f2.rs:12:5"
        );
    }

    #[test]
    fn own_paths_stay_readable() {
        assert_eq!(
            shape_path("/home/jay/.cigscript/ashtray/.flush.lock", &env(), false),
            "$CIGSCRIPT_HOME/ashtray/.flush.lock"
        );
        assert_eq!(
            shape_path("/srv/x/ashtray/.flush.lock", &env(), false),
            "/d1/d2/ashtray/.flush.lock"
        );
        assert_eq!(
            shape_path("/mnt/other/.cigscript/toolchains/cig/1.20.0", &env(), false),
            "/d1/d2/.cigscript/toolchains/cig/1.20.0"
        );
    }

    #[test]
    fn a_second_pass_changes_nothing() {
        let e = env();
        for p in [
            "~/d1/d2/F3.txt",
            "$CIGSCRIPT_HOME/ashtray/.flush.lock",
            "/d1/d2/ashtray/.flush.lock",
            "~/.cigscript/runs/x",
        ] {
            assert_eq!(shape_path(p, &e, false), p, "{p}");
        }
    }

    #[test]
    fn prose_is_not_a_path() {
        assert!(!pathish_in("and/or", false));
        assert!(!pathish_in("yes/no", false));
        assert!(pathish_in("secret/notes.txt", false));
        assert!(pathish_in("a/b/c", false));
        assert!(!pathish_in("src/x.rs", true));
    }

    #[test]
    fn the_fingerprint_ignores_lines_folders_and_machines() {
        let a = fingerprint(
            "ciginstaller",
            "0.1.0",
            "crash",
            "src/cmd/x.rs:12:5",
            "no file /home/a/p/x.cig",
        );
        let b = fingerprint(
            "ciginstaller",
            "0.9.2",
            "crash",
            "src/cmd/x.rs:99:1",
            "no file /Users/b/q/r/x.cig",
        );
        assert_eq!(a, b);
        let c = fingerprint(
            "ciginstaller",
            "1.0.0",
            "crash",
            "src/cmd/x.rs:12:5",
            "no file /home/a/p/x.cig",
        );
        assert_ne!(a, c, "a major version is another fault");
        assert_eq!(a.len(), 8);
    }

    #[test]
    fn a_command_line_is_reduced_to_words_the_installer_knows() {
        know_words(crate::crash::hook::cli_words());
        let got = redact_command(
            [
                "/usr/bin/ciginstaller",
                "install",
                "--home",
                "/home/jay/x",
                "--channel=beta",
                "--token=abc",
                "0.1.2",
                "E1857",
            ]
            .map(String::from),
        );
        assert_eq!(
            got,
            [
                "ciginstaller",
                "install",
                "--home",
                "<path>",
                "--channel=<value>",
                "--token=<redacted>",
                "0.1.2",
                "E1857"
            ]
        );
    }
}
