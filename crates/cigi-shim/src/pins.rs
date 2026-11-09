// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0

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
    let (mut depth, mut ml) = (0usize, None::<&'static str>);
    for line in text.lines() {
        let t = line.trim();
        if depth == 0 && ml.is_none() {
            if t.starts_with('[') {
                return None;
            }
            if let Some(val) = t
                .strip_prefix("carton")
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
