// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! The three project pin-file dialects, read without a parser (SPEC section 6.5).

/// `.cig-carton`: first line that is not blank or a comment; a leading `carton ` is optional.
pub fn pin_from_carton_file(text: &str) -> Option<String> {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))?;
    let rest = match line.strip_prefix("carton") {
        Some(r) if r.starts_with(char::is_whitespace) => r.trim_start(),
        _ => line,
    };
    (!rest.is_empty()).then(|| rest.to_string())
}

/// `cig-tasks.json`: the string value of the top-level key `"carton"`, by a small state machine.
pub fn pin_from_json(text: &str) -> Option<String> {
    top_string(text, "carton")
}

/// `cig-tasks.json`: the string value of the top-level key `"cig"`, the cig the project is written
/// for (cig-orchestrator's manifest, `docs/manifest.md`: `1.2`, `1.2.0` or `>=1.2.0`). The same
/// reader as the carton key, so a nested `"cig"` (a component called cig) never counts.
pub fn cig_from_json(text: &str) -> Option<String> {
    top_string(text, "cig")
}

fn top_string(text: &str, want: &str) -> Option<String> {
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
                if depth == 1 && key == want && b.get(j) == Some(&b':') {
                    j += 1;
                    while j < b.len() && b[j].is_ascii_whitespace() {
                        j += 1;
                    }
                    if b.get(j) != Some(&b'"') {
                        return None;
                    }
                    let end = text[j + 1..].find('"')? + j + 1;
                    let v = &text[j + 1..end];
                    return (!v.contains('\\') && !v.is_empty()).then(|| v.to_string());
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// `cigar.toml`: `carton = "0.1"` before the first table. A line inside a top-level multi-line
/// array, inline table or multi-line string is never taken for a table header or a key.
pub fn pin_from_toml(text: &str) -> Option<String> {
    toml_top_string(text, "carton")
}

/// `cigar.toml`: `cigar = "1.0"` before the first table, the CigarScript version the project is
/// written for. Read by the same scanner as the carton key, so the two can never disagree about
/// what counts as top level. It selects the carton whose cigar meets it, and with it that
/// carton's cigcompile (cig-compile ADR 0003).
pub fn cigar_from_toml(text: &str) -> Option<String> {
    toml_top_string(text, "cigar")
}

fn toml_top_string(text: &str, want: &str) -> Option<String> {
    let (mut depth, mut ml) = (0usize, None::<&'static str>);
    for line in text.lines() {
        let t = line.trim();
        if depth == 0 && ml.is_none() {
            if t.starts_with('[') {
                return None;
            }
            if let Some(val) = t
                .strip_prefix(want)
                .and_then(|r| r.trim_start().strip_prefix('='))
            {
                let val = val.trim();
                let q = val.chars().next().filter(|c| *c == '"' || *c == '\'')?;
                let inner = &val[1..];
                return inner
                    .find(q)
                    .map(|e| inner[..e].to_string())
                    .filter(|s| !s.is_empty());
            }
        }
        scan_toml_line(line.as_bytes(), &mut depth, &mut ml);
    }
    None
}

/// Advance the bracket depth and the multi-line-string state over one line, outside comments and
/// single-line strings.
fn scan_toml_line(b: &[u8], depth: &mut usize, ml: &mut Option<&'static str>) {
    let mut i = 0;
    while i < b.len() {
        if let Some(d) = *ml {
            if b[i..].starts_with(d.as_bytes()) {
                *ml = None;
                i += 3;
            } else if d == "\"\"\"" && b[i] == b'\\' {
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }
        match b[i] {
            b'#' => return,
            q @ (b'"' | b'\'') => {
                let triple: &'static str = if q == b'"' { "\"\"\"" } else { "'''" };
                if b[i..].starts_with(triple.as_bytes()) {
                    *ml = Some(triple);
                    i += 3;
                    continue;
                }
                i += 1;
                while i < b.len() && b[i] != q {
                    i += if q == b'"' && b[i] == b'\\' { 2 } else { 1 };
                }
                i += 1;
                continue;
            }
            b'[' | b'{' => *depth += 1,
            b']' | b'}' => *depth = depth.saturating_sub(1),
            _ => {}
        }
        i += 1;
    }
}

/// A pin a script carries in its header, as cig reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptPin {
    /// What follows `# cig` (or `# cig:`), trimmed: `>=1.6.2`.
    pub text: String,
    /// The line it is on, counting from 1.
    pub line: usize,
}

/// The pin in a script's header: the comment lines before its first statement, a shebang allowed
/// on the first line only, a leading byte-order mark ignored. `# cig 1.1`, `# cig >=1.6.2` and
/// `# cig: 1.1.2` are pins; `# cig run s.cig -- x` (a usage line) and `# cig: run --dry-run`
/// (a directive) are not, because a pin starts like a version. This is the rule of cig's own
/// reader (`src/pin.rs`), so the shim, the installer and cig cannot disagree about which scripts
/// carry one. Whether the text is a constraint cig can read is for the caller.
pub fn header_pin(source: &str) -> Option<ScriptPin> {
    for (i, raw) in source.lines().enumerate() {
        let line = raw.trim_start_matches('\u{feff}').trim();
        if line.is_empty() || (i == 0 && line.starts_with("#!")) {
            continue;
        }
        let Some(comment) = line.strip_prefix('#') else {
            break;
        };
        let comment = comment.trim();
        let Some(rest) = comment
            .strip_prefix("cig:")
            .or_else(|| comment.strip_prefix("cig "))
            .map(str::trim)
        else {
            continue;
        };
        let v = rest.trim_start_matches(">=").trim_start();
        let v = v.strip_prefix('v').unwrap_or(v);
        if v.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            return Some(ScriptPin {
                text: rest.to_string(),
                line: i + 1,
            });
        }
    }
    None
}
