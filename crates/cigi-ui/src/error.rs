// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! `InstallerError` (SPEC section 2.4) and the exit codes of section 11.2.

use crate::registry;

pub const EXIT_OK: i32 = 0;
pub const EXIT_FAILED: i32 = 1;
pub const EXIT_REFUSED: i32 = 2;
pub const EXIT_USAGE: i32 = 3;
pub const EXIT_UNREACHABLE: i32 = 4;
pub const EXIT_NEWER: i32 = 10;
pub const EXIT_BUG: i32 = 70;
pub const EXIT_SHIM: i32 = 127;

/// cig's diagnostic kinds that the installer uses, so cig's `diagnostic.schema.json`
/// accepts the `error` object unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Usage,
    Runtime,
    Internal,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Usage => "usage",
            Kind::Runtime => "runtime",
            Kind::Internal => "internal",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallerError {
    pub code: &'static str,
    pub message: String,
    pub hint: Option<String>,
    pub subject: Option<String>,
    pub kind: Kind,
    pub exit: i32,
    /// A complete command that fixes it (an addition to SPEC 2.4); else the registry's fix is shown.
    pub fix: Option<String>,
}

impl InstallerError {
    /// An error whose kind and exit code come from the registry: the range gives the kind;
    /// the exit follows SPEC 11.2 (refusals 2, usage 3, unreachable 4, bugs 70, else 1).
    pub fn new(code: &'static str, message: impl Into<String>) -> InstallerError {
        InstallerError {
            code,
            message: message.into(),
            hint: None,
            subject: None,
            kind: registry::kind_of(code).unwrap_or(Kind::Runtime),
            exit: default_exit(code),
            fix: None,
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn with_fix(mut self, fix: impl Into<String>) -> Self {
        self.fix = Some(fix.into());
        self
    }

    pub fn with_subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = Some(subject.into());
        self
    }

    pub fn with_exit(mut self, exit: i32) -> Self {
        self.exit = exit;
        self
    }

    pub fn with_kind(mut self, kind: Kind) -> Self {
        self.kind = kind;
        self
    }

    /// The fix to print: the flow's complete command, else the registry's "what to type next".
    pub fn fix_text(&self) -> Option<String> {
        self.fix
            .clone()
            .or_else(|| registry::lookup(self.code).map(|c| c.fix.clone()))
    }
}

/// The default exit code for a code (a flow may override with `with_exit`).
pub fn default_exit(code: &str) -> i32 {
    match code {
        "E1870" | "E1871" => EXIT_USAGE,
        "E1810" | "E1820" | "E1873" => EXIT_UNREACHABLE,
        "E1890" | "E1891" => EXIT_BUG,
        "E1840" => EXIT_SHIM,
        _ => match registry::lookup(code).map(|c| c.no.as_str()) {
            Some("these-are-mine" | "never-heard-of-you" | "cant-see-any" | "dont-have-any") => {
                EXIT_REFUSED
            }
            _ => EXIT_FAILED,
        },
    }
}

impl std::fmt::Display for InstallerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "error[{} installer]: {}", self.code, self.message)
    }
}

impl std::error::Error for InstallerError {}
