// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0
//! `pin carton <pin>`, `pin cig <pin>`; anything else has no pin file (E1843).

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use cig_compat::{Pin, Version};
use cigi_shim::pins::{pin_from_json, pin_from_toml};
use cigi_shim::{read_capped, trusted_entry, trusted_for, Reading, MAX_FILE_INSTALLER};
use cigi_ui::{InstallerError, Report, EXIT_REFUSED, EXIT_USAGE};
use serde_json::json;

use super::Outcome;
use crate::cx::Cx;
use crate::errors::ie;
use crate::project::project_root;

fn usage(msg: String, fix: &str) -> InstallerError {
    InstallerError::new("E1870", msg)
        .with_fix(fix)
        .with_exit(EXIT_USAGE)
}

/// E1841: the existing file cannot be read or understood; nothing is written.
fn unreadable(path: &Path, why: impl std::fmt::Display) -> InstallerError {
    InstallerError::new(
        "E1841",
        format!(
            "cannot read the pin in {}: {why}; nothing was written",
            path.display()
        ),
    )
    .with_fix("fix or remove the file by hand, then run `ciginstaller pin carton <pin>`")
    .with_exit(EXIT_USAGE)
}

fn refused(code: &'static str, msg: String, fix: String) -> InstallerError {
    InstallerError::new(code, msg)
        .with_fix(fix)
        .with_exit(EXIT_REFUSED)
}

fn write_failed(path: &Path, e: impl std::fmt::Display) -> InstallerError {
    InstallerError::new("E1835", format!("cannot write {}: {e}", path.display()))
}

/// What is at a pin target now: nothing, or a plain file (its mode kept). A link or anything that
/// is not a regular file is refused (E1846): the write would follow it or replace it.
fn guard_target(path: &Path) -> Result<Option<fs::Metadata>, InstallerError> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(unreadable(path, e)),
        Ok(md) if md.file_type().is_file() => Ok(Some(md)),
        Ok(md) => Err(refused(
            "E1846",
            format!(
                "{} is {}, not a plain file; pin writes only plain files",
                path.display(),
                if md.file_type().is_symlink() {
                    "a symbolic link"
                } else {
                    "not a regular file"
                }
            ),
            "remove or move it yourself, then run `ciginstaller pin carton <pin>` again".into(),
        )),
    }
}

/// The text of an existing pin file. Only "not there" is an empty start: any other error, and any
/// text that is not UTF-8, is E1841 and nothing is written.
fn read_existing(path: &Path, present: bool) -> Result<Option<String>, InstallerError> {
    if !present {
        return Ok(None);
    }
    let f = match cigi_shim::open_nonblocking(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(unreadable(path, e)),
    };
    // the opened file must still be a plain file (the target was checked before opening)
    if !f.metadata().is_ok_and(|m| m.is_file()) {
        return Err(refused(
            "E1846",
            format!("{} is not a plain file", path.display()),
            "remove or move it yourself, then run `ciginstaller pin carton <pin>` again".into(),
        ));
    }
    let mut bytes = Vec::new();
    f.take(MAX_FILE_INSTALLER + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| unreadable(path, e))?;
    if bytes.len() as u64 > MAX_FILE_INSTALLER {
        return Err(unreadable(path, "it is larger than 1 MiB"));
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| unreadable(path, "it is not UTF-8 text"))
}

/// A name no one can guess: the process id, the clock and a randomly seeded hasher.
fn random_suffix() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u32(std::process::id());
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
    );
    format!("{:016x}", h.finish())
}

/// Write `text` to `path` through a new file beside it: `create_new` (never follows a planted
/// link, never reuses a name), a random name, the old file's mode (0644 under the umask for a new
/// file), fsync, then rename, then fsync of the directory.
fn write_atomic(path: &Path, text: &str, old: Option<&fs::Metadata>) -> Result<(), InstallerError> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let label = crate::project::file_label(path);
    let mut tries = 0;
    let (tmp, mut file) = loop {
        let tmp = dir.join(format!(".{label}.tmp-{}", random_suffix()));
        let mut o = fs::OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // a new file takes 0644 under the umask; an existing file's mode is set below
            o.mode(if old.is_some() { 0o600 } else { 0o644 });
        }
        match o.open(&tmp) {
            Ok(f) => break (tmp, f),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && tries < 16 => tries += 1,
            Err(e) => return Err(write_failed(path, e)),
        }
    };
    let done = (|| -> std::io::Result<()> {
        if let Some(md) = old {
            file.set_permissions(md.permissions())?;
        }
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)
    })();
    if let Err(e) = done {
        let _ = fs::remove_file(&tmp);
        return Err(write_failed(path, e));
    }
    // make the rename durable; not every platform can open a directory
    if let Ok(d) = fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

/// The line ending a file uses: CRLF when it has one, else LF.
fn eol_of(text: &str) -> &'static str {
    if text.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

/// `.cig-carton` (`prefix` = "carton ") and `.cig-version` (`prefix` = ""): replace the pin line,
/// the first that is not blank or a comment, and keep every other line, comments and endings.
fn replace_pin_line(old: Option<&str>, prefix: &str, pin: &str) -> String {
    let text = old.unwrap_or("");
    let eol = eol_of(text);
    let new_line = format!("{prefix}{pin}");
    let mut out = String::with_capacity(text.len() + new_line.len() + 2);
    let mut done = false;
    for l in text.split_inclusive('\n') {
        let body = l.trim_end_matches(['\r', '\n']);
        let t = body.trim();
        if !done && !t.is_empty() && !t.starts_with('#') {
            out.push_str(&new_line);
            out.push_str(&l[body.len()..]);
            done = true;
        } else {
            out.push_str(l);
        }
    }
    if !done {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push_str(eol);
        }
        out.push_str(&new_line);
        out.push_str(eol);
    }
    out
}

/// The end of the JSON value that starts at `i` (a valid document, so no error cases).
fn json_value_end(b: &[u8], mut i: usize) -> usize {
    let skip_string = |mut i: usize| {
        i += 1;
        while i < b.len() && b[i] != b'"' {
            i += if b[i] == b'\\' { 2 } else { 1 };
        }
        i + 1
    };
    match b.get(i) {
        Some(b'"') => skip_string(i),
        Some(b'{' | b'[') => {
            let mut depth = 0usize;
            while i < b.len() {
                match b[i] {
                    b'"' => {
                        i = skip_string(i);
                        continue;
                    }
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth -= 1;
                        if depth == 0 {
                            return i + 1;
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            i
        }
        _ => {
            while i < b.len() && !matches!(b[i], b',' | b'}' | b']') && !b[i].is_ascii_whitespace()
            {
                i += 1;
            }
            i
        }
    }
}

/// The byte range of the value of the top-level key `"carton"` of a valid JSON object.
fn json_carton_value(text: &str) -> Option<(usize, usize)> {
    let b = text.as_bytes();
    let (mut i, mut depth) = (0, 0usize);
    while i < b.len() {
        match b[i] {
            b'{' | b'[' => depth += 1,
            b'}' | b']' => depth = depth.saturating_sub(1),
            b'"' => {
                let start = i + 1;
                i = start;
                while i < b.len() && b[i] != b'"' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                let key = text.get(start..i.min(b.len()))?;
                let mut j = i + 1;
                while j < b.len() && b[j].is_ascii_whitespace() {
                    j += 1;
                }
                if depth == 1 && key == "carton" && b.get(j) == Some(&b':') {
                    j += 1;
                    while j < b.len() && b[j].is_ascii_whitespace() {
                        j += 1;
                    }
                    return Some((j, json_value_end(b, j)));
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// `cig-tasks.json`: replace the top-level `"carton"` value in place, or insert the key after the
/// opening brace; every other byte stays. The result must parse and read back as the pin.
fn json_with_carton(path: &Path, old: Option<&str>, pin: &str) -> Result<String, InstallerError> {
    let Some(text) = old.filter(|t| !t.trim().is_empty()) else {
        return Ok(format!("{{\n  \"carton\": \"{pin}\"\n}}\n"));
    };
    let before: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| unreadable(path, format!("it is not valid JSON ({e})")))?;
    let Some(obj) = before.as_object() else {
        return Err(unreadable(path, "it is not a JSON object"));
    };
    let quoted = format!("\"{pin}\"");
    let result = if let Some((s, e)) = json_carton_value(text) {
        format!("{}{quoted}{}", &text[..s], &text[e..])
    } else {
        let open = text
            .find('{')
            .ok_or_else(|| unreadable(path, "it is not a JSON object"))?
            + 1;
        let rest = &text[open..];
        let eol = eol_of(text);
        let comma = if obj.is_empty() { "" } else { "," };
        let after_ws = rest.trim_start_matches([' ', '\t']);
        let insert = if after_ws.starts_with('\n') || after_ws.starts_with("\r\n") {
            // pretty-printed: the new key takes the indent of the next line
            let next = after_ws.trim_start_matches(['\r', '\n']);
            let indent = &next[..next.len() - next.trim_start_matches([' ', '\t']).len()];
            format!("{eol}{indent}\"carton\": {quoted}{comma}")
        } else if obj.is_empty() {
            format!("\"carton\": {quoted}")
        } else {
            format!("\"carton\": {quoted}, ")
        };
        format!("{}{insert}{}", &text[..open], rest)
    };
    let after: serde_json::Value =
        serde_json::from_str(&result).map_err(|e| not_readable_back(path, e))?;
    let mut expect = before;
    if let Some(o) = expect.as_object_mut() {
        o.insert("carton".into(), json!(pin));
    }
    if after != expect || pin_from_json(&result).as_deref() != Some(pin) {
        return Err(not_readable_back(
            path,
            "the carton key is not where the shim reads it",
        ));
    }
    Ok(result)
}

fn not_readable_back(path: &Path, why: impl std::fmt::Display) -> InstallerError {
    refused(
        "E1848",
        format!(
            "the edit to {} would not read back ({why}); nothing was written",
            path.display()
        ),
        "edit the carton line by hand, or run `ciginstaller pin carton <pin>`".into(),
    )
}

/// `cigar.toml`: set the top-level `carton` key with `toml_edit`: the old value's trailing
/// comment, every other byte of formatting and comments, and CRLF endings stay.
fn toml_with_carton(path: &Path, old: Option<&str>, pin: &str) -> Result<String, InstallerError> {
    let text = old.unwrap_or("");
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e| unreadable(path, format!("it is not valid TOML ({e})")))?;
    match doc.get_mut("carton") {
        Some(item) => {
            let Some(v) = item.as_value_mut() else {
                return Err(unreadable(path, "`carton` is a table, not a value"));
            };
            let mut nv = toml_edit::Value::from(pin);
            *nv.decor_mut() = v.decor().clone();
            *v = nv;
        }
        None => {
            doc.insert("carton", toml_edit::value(pin));
        }
    }
    let mut out = doc.to_string();
    // a file whose every line ends in CRLF keeps it, the new line included
    if text.contains("\r\n") && !text.replace("\r\n", "").contains('\n') {
        out = out.replace("\r\n", "\n").replace('\n', "\r\n");
    }
    if out != text && !out.ends_with('\n') {
        out.push_str(eol_of(text));
    }
    if pin_from_toml(&out).as_deref() != Some(pin) {
        return Err(not_readable_back(
            path,
            "the carton key is not where the shim reads it",
        ));
    }
    Ok(out)
}

/// A pin file in the same directory that the shim reads before `target` (E1847).
fn outranked_by(dir: &Path, target: &str, cigscript_home: &Path) -> Option<&'static str> {
    let t = trusted_for(cigscript_home);
    let carton = dir.join(".cig-carton");
    if target != ".cig-carton" && trusted_entry(&carton, t, false) {
        return Some(".cig-carton");
    }
    if target == "cigar.toml" {
        match read_capped(&dir.join("cig-tasks.json"), t, MAX_FILE_INSTALLER) {
            Reading::Absent => {}
            // unusable: the shim stops there too
            Reading::Unusable(_) => return Some("cig-tasks.json"),
            Reading::Text(s) => {
                if pin_from_json(&s).is_some() {
                    return Some("cig-tasks.json");
                }
            }
        }
    }
    None
}

pub fn run(
    cx: &mut Cx,
    tool: &str,
    pin: &str,
    file: Option<&str>,
) -> Result<Outcome, InstallerError> {
    // pin files live in the project, but the home's permissions are still checked before any write
    cx.home.check_permissions().map_err(ie)?;
    if tool != "carton" && tool != "cig" {
        return Err(InstallerError::new(
            "E1843",
            format!("there is no per-tool pin file for `{tool}`; pin the carton that holds the build you want"),
        )
        .with_fix(format!("ciginstaller pin carton <pin>  # the carton that holds {tool}"))
        .with_exit(EXIT_REFUSED));
    }
    let parsed =
        Pin::parse(pin).map_err(|e| usage(e.to_string(), "ciginstaller pin carton 0.1"))?;
    let home_root = cx.home.root();
    let root = project_root(&cx.cwd, &home_root, cigi_shim::user_home().as_deref());
    let pin_text = parsed.to_string();
    let (name, what) = match (tool, file) {
        ("cig", _) => (".cig-version", "cig"),
        (_, None) => (".cig-carton", "carton"),
        (_, Some("cig-tasks.json")) => ("cig-tasks.json", "carton"),
        (_, Some("cigar.toml")) => ("cigar.toml", "carton"),
        (_, Some(other)) => {
            return Err(usage(
                format!("`--file {other}` is not cig-tasks.json or cigar.toml"),
                "ciginstaller pin carton 0.1 --file cig-tasks.json",
            ))
        }
    };
    let path: PathBuf = root.join(name);
    let old_md = guard_target(&path)?;
    if what == "carton" {
        if let Some(first) = outranked_by(&root, name, &home_root) {
            return Err(refused(
                "E1847",
                format!(
                    "{first} in {} is read before {name}, so a pin written to {name} would never be used",
                    root.display()
                ),
                "ciginstaller pin carton <pin>  # updates .cig-carton, or remove the outranking pin yourself".into(),
            ));
        }
    }
    let old = read_existing(&path, old_md.is_some())?;
    let text = match name {
        ".cig-version" => replace_pin_line(old.as_deref(), "", &pin_text),
        ".cig-carton" => replace_pin_line(old.as_deref(), "carton ", &pin_text),
        "cig-tasks.json" => json_with_carton(&path, old.as_deref(), &pin_text)?,
        _ => toml_with_carton(&path, old.as_deref(), &pin_text)?,
    };
    write_atomic(&path, &text, old_md.as_ref())?;
    let mut r = Report::done(format!("{what} pinned to {parsed} in {}", path.display()));
    if what == "carton" {
        let fits = cx
            .state
            .cartons
            .keys()
            .filter_map(|k| Version::parse(k).ok())
            .any(|c| parsed.satisfied_by(&c));
        if !fits {
            r = r.detail(format!(
                "no installed carton fits {parsed} yet; the first run fetches one (pin_fetch)"
            ));
        }
    }
    Ok(
        r.data(json!({ "pin": parsed.to_string(), "file": path.display().to_string() }))
            .into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: &str = "f";

    #[test]
    fn pin_line_keeps_comments_and_endings() {
        assert_eq!(replace_pin_line(None, "carton ", "0.2"), "carton 0.2\n");
        assert_eq!(
            replace_pin_line(Some("# team\r\ncarton 0.1\r\n# end\r\n"), "carton ", "0.2"),
            "# team\r\ncarton 0.2\r\n# end\r\n"
        );
        assert_eq!(
            replace_pin_line(Some("# only a comment"), "carton ", "0.2"),
            "# only a comment\ncarton 0.2\n"
        );
        assert_eq!(replace_pin_line(Some("0.1"), "", "0.3"), "0.3");
        assert_eq!(
            replace_pin_line(Some("\n  carton 0.1  \n"), "carton ", "1"),
            "\ncarton 1\n"
        );
    }

    #[test]
    fn json_edit_is_targeted() {
        let p = Path::new(P);
        let e = |t: &str| json_with_carton(p, Some(t), "0.2").map_err(|e| e.code);
        assert_eq!(
            e("{\n  \"a\": {\"carton\": \"9\"},\n  \"carton\": \"0.1\",\n  \"n\": [1]\n}\n")
                .unwrap(),
            "{\n  \"a\": {\"carton\": \"9\"},\n  \"carton\": \"0.2\",\n  \"n\": [1]\n}\n"
        );
        assert_eq!(
            e("{\r\n\t\"a\": 1\r\n}\r\n").unwrap(),
            "{\r\n\t\"carton\": \"0.2\",\r\n\t\"a\": 1\r\n}\r\n"
        );
        assert_eq!(e("{\"a\":1}").unwrap(), "{\"carton\": \"0.2\", \"a\":1}");
        assert_eq!(e("{}").unwrap(), "{\"carton\": \"0.2\"}");
        assert_eq!(
            e("{\"carton\": 7, \"b\": 1}").unwrap(),
            "{\"carton\": \"0.2\", \"b\": 1}"
        );
        assert_eq!(e("[1]"), Err("E1841"));
        assert_eq!(e("{\"a\": "), Err("E1841"));
        assert_eq!(
            json_with_carton(p, None, "0.2").unwrap(),
            "{\n  \"carton\": \"0.2\"\n}\n"
        );
    }

    #[test]
    fn toml_edit_keeps_everything_else() {
        let p = Path::new(P);
        let e = |t: &str| toml_with_carton(p, Some(t), "0.2").map_err(|e| e.code);
        assert_eq!(
            e("m = [\n  [1, 2],\n  [3, 4],\n]\nname = \"x\" # keep\n[deps]\nx = 1\n").unwrap(),
            "m = [\n  [1, 2],\n  [3, 4],\n]\nname = \"x\" # keep\ncarton = \"0.2\"\n[deps]\nx = 1\n"
        );
        assert_eq!(
            e("# top\ncarton = \"0.1\" # the pin\nname = 'x'\n").unwrap(),
            "# top\ncarton = \"0.2\" # the pin\nname = 'x'\n"
        );
        assert_eq!(
            e("name = \"x\"\r\ncarton = '0.1'  # c\r\n[d]\r\nx = 1\r\n").unwrap(),
            "name = \"x\"\r\ncarton = \"0.2\"  # c\r\n[d]\r\nx = 1\r\n"
        );
        let crlf_new = e("name = \"x\"\r\n[d]\r\nx = 1\r\n").unwrap();
        assert!(!crlf_new.replace("\r\n", "").contains('\n'), "{crlf_new:?}");
        assert!(crlf_new.contains("carton = \"0.2\"\r\n"));
        assert_eq!(e("").unwrap(), "carton = \"0.2\"\n");
        assert_eq!(e("[carton]\nx = 1\n"), Err("E1841"));
        assert_eq!(e("a = = 1"), Err("E1841"));
    }
}
