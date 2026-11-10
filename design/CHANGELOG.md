<!-- SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer) -->
<!-- SPDX-License-Identifier: CC-BY-4.0 -->

# Changelog of the design standard

The standard is versioned with semver. A product declares the version it meets, and the standards sweep (#34) checks it. A new token, role or screen is a minor version; changing or removing one is a major version and ships with a migration; a fix to wording or a measured value is a patch.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- A neutral theme for a terminal whose background is not found: text stays at the terminal's foreground and only glyphs are colored, from a palette at 3:1 or better on both black and white.
- 16-color codes per theme (`ansi16.dark`, `ansi16.light`, `ansi16.neutral` in `tokens.json`), chosen from the standard xterm palette and tested at 4.5:1.
- `action:` for a finding that needs something done, in `warn`; plain is also automatic in a non-English locale.
- The `ascii` variant of every screen's golden.
- `docs_house_style`, the sweep's checks run on this repository.

### Changed

- Plain names the kinds of no by the manual's names.
- An irreversible question takes a typed `yes`; a prompt in plain is a typed line; three answers that are not options stop the run with exit 3.
- Tables are cut only for a person at a terminal in the voice.
- The progress bar's track is a different glyph from its fill.

## [1.0.0]

Released with carton 1.0.0 (#25). Written first in the 0.1 line of ciginstaller, taken from the assets the family already has.

### Added

- `tokens.json`, the source of every color, size and timing, in the W3C Design Tokens format, with contrast measured for each pair a component draws.
- New tokens: `ember-ink` (`#c2410c`, ember as text on light), `ok` light (`#15803d`), `warn` (`#fbbf24` dark, `#b45309` light), `danger` (`#f87171` dark, `#b91c1c` light), `info`.
- Generated from the tokens, never hand-copied: `crates/cigi-ui/src/tokens.rs`, `generated/tokens.css`, `generated/egui-theme.rs`, `generated/terminal-palettes.json`, `generated/svg-styles.css`, `generated/badges.json`. `cargo xtask tokens` builds them and `generated_files_match_tokens` fails on drift.
- The philosophy, the voice and plain with fifty paired examples, the accessibility bar, the house style for docs, and the components.
- The four kinds of no mapped to roles, so a refusal is painted by its kind.
- The terminal UI of #32: the ASCII icon set, light-terminal detection, `--wide`, one-keypress answers, `error[E1811 carton]` labels with the `= doctor:` line, boxes, and the first-line budgets.

### Changed

- Ember is never text on a light background.
- A refusal is no longer always red.
- Product names in prose are CigCompile and CigarScript.

### Known limits

- A screen reader cannot be detected from the environment; plain is set with `CIG_SCREEN_READER` or `voice = plain` (see accessibility.md).
- The shared `cig-paint` crate of #32 lives in cig-compile; until it exists, roles and the painter live in `cigi-ui` (`tokens.rs`, `paint.rs`) behind an interface that can be lifted out as it stands.
