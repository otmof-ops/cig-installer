// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! Blame, not only origin (#28 section 3): where a fault is, as against where it was seen.
//!
//! - a crash in cigo whose top frame is in cig is cig's bug;
//! - a compiled program crashing inside the runtime is cig's;
//! - a crash in the installer while running a product's trial is the product's.
//!
//! Decided from what the report already carries: the top frame that names a product, else the
//! error code's range (#29 lists them), else the origin. Nothing here reads anything else.

use serde_json::Value;

use super::repo_of_origin;

/// A product of the family, by the name its repository uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Product {
    Cig,
    Cigo,
    CigCompile,
    Cigar,
    Installer,
}

impl Product {
    pub fn word(self) -> &'static str {
        match self {
            Product::Cig => "cig",
            Product::Cigo => "cig-orchestrator",
            Product::CigCompile => "cig-compile",
            Product::Cigar => "cigar-script",
            Product::Installer => "ciginstaller",
        }
    }
    pub fn repo(self) -> &'static str {
        repo_of_origin(self.word())
    }
    pub fn of_origin(origin: &str) -> Option<Product> {
        Some(match origin {
            "cig" | "cig-script" => Product::Cig,
            "cigo" | "cig-orchestrator" => Product::Cigo,
            "cigcompile" | "cig-compile" => Product::CigCompile,
            "cigar" | "cigar-script" | "cigarscript" => Product::Cigar,
            "ciginstaller" | "cig-installer" => Product::Installer,
            _ => return None,
        })
    }
}

/// How the blame was decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Via {
    Frames,
    Code,
    Origin,
}

impl Via {
    pub fn word(self) -> &'static str {
        match self {
            Via::Frames => "frames",
            Via::Code => "code",
            Via::Origin => "origin",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Blame {
    /// The blamed product's name, or the origin's own word when it is not a family product.
    pub product: String,
    /// `owner/name`.
    pub repo: String,
    pub via: Via,
}

/// Which product owns an error code (`E900` is the number 900).
pub fn product_of_code(n: u32) -> Option<Product> {
    match n {
        870..=899 => Some(Product::Cigo),
        1000..=1099 => Some(Product::CigCompile),
        1800..=1999 => Some(Product::Installer),
        100..=869 | 900..=999 => Some(Product::Cig),
        _ => None,
    }
}

/// The first `E<digits>` code in a text, as a number.
pub fn first_code(text: &str) -> Option<u32> {
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let boundary = i == 0 || !b[i - 1].is_ascii_alphanumeric();
        if b[i] == b'E' && boundary {
            let digits: String = text[i + 1..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            if (3..=4).contains(&digits.len()) {
                let after = text[i + 1 + digits.len()..].chars().next();
                if !after.is_some_and(|c| c.is_ascii_alphanumeric()) {
                    return digits.parse().ok();
                }
            }
        }
        i += 1;
    }
    None
}

/// The product a stack frame is in, by the crate or folder it names.
pub fn product_of_frame(frame: &str) -> Option<Product> {
    let f = frame.to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| f.contains(n));
    // longer, more specific names first: `cig_compile` before `cig`
    if has(&["cig_compile", "cig-compile", "cigcompile"]) {
        Some(Product::CigCompile)
    } else if has(&[
        "cig_orchestrator",
        "cig-orchestrator",
        "cigo::",
        "cigo/",
        "orchestrate.cig",
    ]) {
        Some(Product::Cigo)
    } else if has(&["cigar_script", "cigar-script", "cigarscript"]) {
        Some(Product::Cigar)
    } else if has(&["ciginstaller", "cigi_", "cig-installer"]) {
        Some(Product::Installer)
    } else if has(&[
        "cig_script",
        "cig-script",
        "cig_runtime",
        "cigrt",
        "cig::",
        "cig/",
    ]) {
        Some(Product::Cig)
    } else {
        None
    }
}

/// Decide the blame for a report as it will travel (`travel`'s object).
pub fn of(view: &Value) -> Blame {
    let origin = view["origin"].as_str().unwrap_or("cig");
    let blamed = |p: Product, via: Via| Blame {
        product: p.word().to_string(),
        repo: p.repo().to_string(),
        via,
    };
    if let Some(p) = view["backtrace"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .find_map(product_of_frame)
    {
        return blamed(p, Via::Frames);
    }
    for key in ["panic_location", "panic_message", "title"] {
        if let Some(p) = view[key]
            .as_str()
            .and_then(first_code)
            .and_then(product_of_code)
        {
            return blamed(p, Via::Code);
        }
    }
    match Product::of_origin(origin) {
        Some(p) => blamed(p, Via::Origin),
        None => Blame {
            product: origin.to_string(),
            repo: repo_of_origin(origin).to_string(),
            via: Via::Origin,
        },
    }
}

/// The repository the origin itself would have filed in.
pub fn origin_repo(view: &Value) -> String {
    repo_of_origin(view["origin"].as_str().unwrap_or("cig")).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn code_ranges() {
        assert_eq!(product_of_code(900), Some(Product::Cig));
        assert_eq!(product_of_code(875), Some(Product::Cigo));
        assert_eq!(product_of_code(1010), Some(Product::CigCompile));
        assert_eq!(product_of_code(1857), Some(Product::Installer));
        assert_eq!(product_of_code(5), None);
        assert_eq!(first_code("raised E900 in the burn"), Some(900));
        assert_eq!(first_code("HE900 or E90000"), None);
    }

    #[test]
    fn frames_then_code_then_origin() {
        // the top frame is in cig: cig's bug, though cigo saw it
        let v = json!({"origin": "cig-orchestrator", "backtrace": ["cig::builtins::raise", "cigo::run"], "panic_location": "E875"});
        let b = of(&v);
        assert_eq!((b.product.as_str(), b.via), ("cig", Via::Frames));
        assert_eq!(b.repo, "otmof-ops/cig-script");
        // no frames: the code's range
        let v = json!({"origin": "cig-orchestrator", "backtrace": [], "panic_location": "E900"});
        assert_eq!(of(&v).via, Via::Code);
        assert_eq!(of(&v).product, "cig");
        // nothing else to go on: where it was seen
        let v = json!({"origin": "cig-compile", "backtrace": ["main"], "panic_message": "boom"});
        let b = of(&v);
        assert_eq!((b.product.as_str(), b.via), ("cig-compile", Via::Origin));
    }
}
