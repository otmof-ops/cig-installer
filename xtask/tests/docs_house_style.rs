// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! `docs_house_style` (#31): the sweep's checks for the house style in `design/docs.md`, run on
//! this repository first. It covers `docs/`, `design/` and the repository's own top-level
//! pages:
//!
//! * every page has the CC-BY-4.0 header, or `REUSE.toml` maps it to CC-BY-4.0;
//! * one H1 (the README's title is its logo);
//! * sentence-case headings;
//! * every code block names its language;
//! * no screenshots of text (images are SVG);
//! * American English;
//! * the README follows the house order;
//! * the files every repository has, with the logo and a 1280 by 640 social preview, and `YANKED` once there are releases.

use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn markdown_in(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            markdown_in(&p, out);
        } else if p.extension().is_some_and(|x| x == "md") {
            out.push(p);
        }
    }
}

/// The pages the house style covers.
fn pages() -> Vec<PathBuf> {
    let root = root();
    let mut v = Vec::new();
    markdown_in(&root.join("docs"), &mut v);
    markdown_in(&root.join("design"), &mut v);
    for name in [
        "README.md",
        "CONTRIBUTING.md",
        "SECURITY.md",
        "SUPPORT.md",
        "TRADEMARKS.md",
        "LICENSING.md",
        "THIRD-PARTY-NOTICES.md",
    ] {
        let p = root.join(name);
        if p.is_file() {
            v.push(p);
        }
    }
    v.sort();
    v
}

fn rel(p: &Path) -> String {
    p.strip_prefix(root())
        .unwrap_or(p)
        .to_string_lossy()
        .into_owned()
}

/// Lines of a page with their fence state: `(line, in_code_block, is_fence_line)`.
fn lines_of(text: &str) -> Vec<(&str, bool, bool)> {
    let mut fenced = false;
    text.lines()
        .map(|l| {
            if l.trim_start().starts_with("```") {
                fenced = !fenced;
                (l, true, true)
            } else {
                (l, fenced, false)
            }
        })
        .collect()
}

#[test]
fn every_page_is_licensed_cc_by_4_0() {
    let reuse: toml::Value = read(&root().join("REUSE.toml"))
        .parse()
        .expect("REUSE.toml");
    let mapped = |path: &str| {
        reuse
            .get("annotations")
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten()
            .filter(|a| {
                a.get("SPDX-License-Identifier")
                    .and_then(toml::Value::as_str)
                    == Some("CC-BY-4.0")
            })
            .flat_map(|a| {
                a.get("path")
                    .and_then(toml::Value::as_array)
                    .into_iter()
                    .flatten()
            })
            .filter_map(toml::Value::as_str)
            .any(|pat| match pat.strip_suffix("/**") {
                Some(dir) => path.starts_with(&format!("{dir}/")),
                None => pat == path,
            })
    };
    for p in pages() {
        let text = read(&p);
        let head: String = text.lines().take(3).collect::<Vec<_>>().join("\n");
        let header = head.contains("SPDX-License-Identifier: CC-BY-4.0");
        assert!(
            header || mapped(&rel(&p)),
            "{} has no CC-BY-4.0 header and REUSE.toml does not map it",
            rel(&p)
        );
    }
}

#[test]
fn one_h1_per_page() {
    for p in pages() {
        if rel(&p) == "README.md" {
            continue; // the README's title is the logo
        }
        let text = read(&p);
        let h1s = lines_of(&text)
            .iter()
            .filter(|(l, code, _)| !code && l.starts_with("# "))
            .count();
        assert_eq!(h1s, 1, "{} has {h1s} H1s", rel(&p));
    }
}

/// Words that keep their capitals in a sentence-case heading.
const PROPER: &[&str] = &[
    "CigScript",
    "CigCompile",
    "CigarScript",
    "Cig",
    "Keep",
    "Changelog",
    "JSON",
    "README",
    "Rust",
    "GitHub",
    "OSC",
    "ASCII",
    "API",
    "TTY",
    "CI",
    "Linux",
    "Windows",
    "macOS",
    "DCO",
    "SPDX",
    "REUSE",
    "ADR",
    "FSL",
    "ALv2",
    "Apache",
    "Inter",
    "JetBrains",
    "Mono",
    "AccessKit",
    "Unicode",
    "NO_COLOR",
    "PATH",
    "HTTP",
    "HTTPS",
    "OS",
    "UI",
    "CLI",
    "SVG",
    "CSS",
    "W3C",
    "Design",
    "Tokens",
    "English",
    "Australian",
    "ember",
    "Zlib",
    "MIT",
    "LLVM-exception",
    "URLs",
    "CIG",
    "OS-level",
    "E1800",
];

fn is_title_case_word(w: &str) -> bool {
    let w = w.trim_matches(|c: char| !c.is_alphanumeric());
    let mut chars = w.chars();
    chars.next().is_some_and(char::is_uppercase)
        && w.chars().skip(1).any(char::is_lowercase)
        // license identifiers and version numbers (`Apache-2.0`) keep their form
        && !w.chars().any(|c| c.is_ascii_digit())
        && !PROPER.contains(&w)
}

#[test]
fn headings_are_sentence_case() {
    for p in pages() {
        let text = read(&p);
        for (l, code, _) in lines_of(&text) {
            if code || !l.starts_with('#') {
                continue;
            }
            let title = l.trim_start_matches('#').trim();
            // words after the first, ignoring anything in code spans, product names and numbers
            let plain: String = title.split('`').step_by(2).collect::<Vec<_>>().join(" ");
            // a number or decision id in front ("3.", "0001:") is not the first word
            let bad: Vec<&str> = plain
                .split_whitespace()
                .skip_while(|w| {
                    w.trim_end_matches(['.', ':'])
                        .chars()
                        .all(|c| c.is_ascii_digit())
                })
                .skip(1)
                .filter(|w| is_title_case_word(w))
                .collect();
            assert!(
                bad.is_empty(),
                "{}: heading `{title}` is not sentence case ({bad:?})",
                rel(&p)
            );
        }
    }
}

#[test]
fn every_code_block_names_its_language() {
    for p in pages() {
        let text = read(&p);
        let mut open = false;
        for (n, (l, _, fence)) in lines_of(&text).into_iter().enumerate() {
            if !fence {
                continue;
            }
            if !open {
                let lang = l.trim_start().trim_start_matches('`').trim();
                assert!(
                    !lang.is_empty(),
                    "{}:{}: a code block with no language",
                    rel(&p),
                    n + 1
                );
            }
            open = !open;
        }
    }
}

#[test]
fn no_screenshots_of_text() {
    for p in pages() {
        let text = read(&p);
        for (n, (l, code, _)) in lines_of(&text).into_iter().enumerate() {
            if code {
                continue;
            }
            for ext in [".png", ".jpg", ".jpeg", ".gif", ".webp"] {
                // the social preview is the one raster image the family ships
                let allowed = l.contains("social-preview");
                assert!(
                    !l.to_ascii_lowercase().contains(ext) || allowed,
                    "{}:{}: an image of {ext}; render terminals as SVG",
                    rel(&p),
                    n + 1
                );
            }
        }
    }
}

#[test]
fn american_english() {
    let banned = [
        "colour",
        "behaviour",
        "honour",
        "favour",
        "licence",
        "organis",
        "recognis",
        "centre",
        "grey",
        "localis",
        "initialis",
        "analyse",
        "cancelled",
        "artefact",
    ];
    for p in pages() {
        let text = read(&p);
        for (n, (l, code, _)) in lines_of(&text).into_iter().enumerate() {
            if code {
                continue;
            }
            // text inside backticks is a command or a file name, written as it is
            let prose: String = l
                .split('`')
                .step_by(2)
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase();
            for w in banned {
                assert!(
                    !prose.contains(w),
                    "{}:{}: `{w}` is not American English",
                    rel(&p),
                    n + 1
                );
            }
        }
    }
}

#[test]
fn the_readme_follows_the_house_order() {
    let text = read(&root().join("README.md"));
    // 1. the logo, centered, first
    let first = text
        .lines()
        .find(|l| !l.trim().is_empty() && !l.trim_start().starts_with("<!--"))
        .unwrap_or("");
    assert!(
        first.contains("align=\"center\""),
        "the logo comes first, centered: {first}"
    );
    assert!(text.contains("docs/assets/logo.svg"));
    // 2 and 3. a bold promise, an italic second line, badges
    assert!(
        text.contains("<b>") && text.contains("<i>"),
        "a bold promise and an italic line"
    );
    // 5 to 11, in order
    let h2: Vec<String> = lines_of(&text)
        .iter()
        .filter(|(l, code, _)| !code && l.starts_with("## "))
        .map(|(l, _, _)| l.to_lowercase())
        .collect();
    let mut at = 0;
    for key in [
        "tour",
        "what you get",
        "install",
        "commands",
        "limits",
        "part of",
        "licens",
    ] {
        let found = h2.iter().skip(at).position(|h| h.contains(key));
        let found =
            found.unwrap_or_else(|| panic!("the README has no `{key}` section in order: {h2:?}"));
        at += found + 1;
    }
}

#[test]
fn the_files_every_repository_has_exist() {
    let root = root();
    for f in [
        "README.md",
        "CHANGELOG.md",
        "CONTRIBUTING.md",
        "SECURITY.md",
        "SUPPORT.md",
        "TRADEMARKS.md",
        "LICENSE",
        "NOTICE",
        "THIRD-PARTY-NOTICES.md",
        "REUSE.toml",
        "docs/assets/logo.svg",
    ] {
        assert!(root.join(f).is_file(), "{f} is missing");
    }
    assert!(root.join("LICENSES").is_dir(), "LICENSES/ is missing");
    // YANKED once there is a release
    let changelog = read(&root.join("CHANGELOG.md"));
    let released = changelog
        .lines()
        .any(|l| l.starts_with("## [") && l[4..].starts_with(|c: char| c.is_ascii_digit()));
    if released {
        assert!(
            root.join("YANKED").is_file(),
            "there are releases, so YANKED is needed"
        );
    }
}

#[test]
fn the_social_preview_is_1280_by_640() {
    let dir = root().join("docs/assets");
    let svg = dir.join("social-preview.svg");
    let png = dir.join("social-preview.png");
    assert!(
        svg.is_file() || png.is_file(),
        "no social preview in docs/assets"
    );
    if svg.is_file() {
        let text = read(&svg);
        let attr = |name: &str| -> Option<u32> {
            let at = text.find(&format!("{name}=\""))? + name.len() + 2;
            text[at..]
                .split('"')
                .next()?
                .trim_end_matches("px")
                .parse()
                .ok()
        };
        assert_eq!(
            (attr("width"), attr("height")),
            (Some(1280), Some(640)),
            "{}",
            rel(&svg)
        );
    }
    if png.is_file() {
        // the PNG header: 8 signature bytes, a 4-byte length, `IHDR`, then width and height
        let b = std::fs::read(&png).expect("png");
        assert!(b.len() > 24 && &b[12..16] == b"IHDR", "not a PNG");
        let w = u32::from_be_bytes([b[16], b[17], b[18], b[19]]);
        let h = u32::from_be_bytes([b[20], b[21], b[22], b[23]]);
        assert_eq!((w, h), (1280, 640), "{}", rel(&png));
    }
}
