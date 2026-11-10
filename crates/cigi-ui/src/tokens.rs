// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2

//! The family palette as roles (SPEC section 2.4, issue #32).
//!
//! TEMPORARY until #31 lands `design/tokens.json`: this table is the single place the
//! colour values live. Ember comes from the SVG assets in cig-script's `docs/assets`
//! (`#f97316`, `#fb923c`, `#ea580c`); ash is the grey those assets use for secondary text.
//! `ok`, `warn` and `danger` are placeholders in the same family of values.

/// How many colours the terminal can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    True,
    Ansi256,
    Ansi16,
    None,
}

/// Which background the colours are chosen for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    Dark,
    Light,
}

/// Roles, never raw colour codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Ok,
    Warn,
    Danger,
    Ember,
    EmberBright,
    Ash400,
    EmberInk,
    Bold,
}

/// One role's value at each depth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub rgb: (u8, u8, u8),
    pub ansi256: u8,
    /// SGR parameters for the 16-colour fallback.
    pub ansi16: &'static str,
}

const fn t(r: u8, g: u8, b: u8, ansi256: u8, ansi16: &'static str) -> Token {
    Token {
        rgb: (r, g, b),
        ansi256,
        ansi16,
    }
}

/// The token for a role on a theme. `Bold` has no colour; its token is unused (see `sgr`).
pub fn token(role: Role, theme: Theme) -> Token {
    match (theme, role) {
        (Theme::Dark, Role::Ok) => t(0x22, 0xc5, 0x5e, 41, "32"),
        (Theme::Dark, Role::Warn) => t(0xea, 0xb3, 0x08, 178, "33"),
        (Theme::Dark, Role::Danger) => t(0xef, 0x44, 0x44, 203, "31"),
        (Theme::Dark, Role::Ember) => t(0xf9, 0x73, 0x16, 208, "93"),
        (Theme::Dark, Role::EmberBright) => t(0xfb, 0x92, 0x3c, 214, "93"),
        (Theme::Dark, Role::Ash400) => t(0x9c, 0xa3, 0xaf, 145, "2"),
        (Theme::Dark, Role::EmberInk) => t(0xea, 0x58, 0x0c, 166, "93"),
        (Theme::Light, Role::Ok) => t(0x16, 0xa3, 0x4a, 34, "32"),
        (Theme::Light, Role::Warn) => t(0xa1, 0x62, 0x07, 136, "33"),
        (Theme::Light, Role::Danger) => t(0xdc, 0x26, 0x26, 160, "31"),
        (Theme::Light, Role::Ember | Role::EmberInk) => t(0xea, 0x58, 0x0c, 166, "93"),
        (Theme::Light, Role::EmberBright) => t(0xc2, 0x41, 0x0c, 130, "93"),
        (Theme::Light, Role::Ash400) => t(0x6b, 0x72, 0x80, 243, "2"),
        (_, Role::Bold) => t(0, 0, 0, 0, "1"),
    }
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
