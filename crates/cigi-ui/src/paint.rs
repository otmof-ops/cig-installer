// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! The painter (#32): a role and a theme in, an escape sequence out, at the depth the terminal
//! has. This module and `tokens.rs` are the part of `cigi-ui` meant to be lifted into the
//! family's shared `cig-paint` crate (#32's order of work, step 1) once that crate exists in
//! cig-compile; nothing here knows about the installer.

use crate::tokens::{token, Role, Theme};

/// How many colours the terminal can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    True,
    Ansi256,
    Ansi16,
    None,
}

/// The opening escape for a role, or `None` when colour is off.
pub fn sgr(mode: ColorMode, theme: Theme, role: Role) -> Option<String> {
    if mode == ColorMode::None {
        return None;
    }
    if role == Role::Bold {
        return Some("\x1b[1m".to_string());
    }
    let tok = token(role, theme);
    Some(match mode {
        ColorMode::True => format!("\x1b[38;2;{};{};{}m", tok.rgb.0, tok.rgb.1, tok.rgb.2),
        ColorMode::Ansi256 => format!("\x1b[38;5;{}m", tok.ansi256),
        _ => format!("\x1b[{}m", tok.ansi16),
    })
}

/// Wrap `text` in a role's colour. With colour off the text is returned unchanged.
pub fn paint(mode: ColorMode, theme: Theme, role: Role, text: &str) -> String {
    match sgr(mode, theme, role) {
        Some(open) if !text.is_empty() => format!("{open}{text}\x1b[0m"),
        _ => text.to_string(),
    }
}
