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

/// `cigar.toml`: `carton = "0.1"` before the first table.
pub fn pin_from_toml(text: &str) -> Option<String> {
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            return None;
        }
        let Some(rest) = line.strip_prefix("carton") else {
            continue;
        };
        let Some(val) = rest.trim_start().strip_prefix('=') else {
            continue;
        };
        let val = val.trim();
        let q = val.chars().next().filter(|c| *c == '"' || *c == '\'')?;
        let inner = &val[1..];
        return inner
            .find(q)
            .map(|e| inner[..e].to_string())
            .filter(|s| !s.is_empty());
    }
    None
}
