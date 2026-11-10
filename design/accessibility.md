<!-- SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer) -->
<!-- SPDX-License-Identifier: CC-BY-4.0 -->

# Accessibility

Standard version 1.0.0. The bar, and how each surface meets it.

## The bar

- **Text contrast** is at least 4.5:1 for body text and at least 3:1 for large text and UI components, in both themes. The test `every_token_pair_used_meets_contrast` (crates/cigi-ui/tests/contrast.rs) checks every pair listed under `pairs` in [tokens.json](tokens.json), and `every_terminal_role_is_text_that_reads_on_its_theme` checks every terminal role against both backgrounds.
- **Never colour alone.** Every state has an icon and a word. The icons are `✓ ● ? ✗ ! ·` and, in ASCII, `[ok] [..] [?] [x] [!] [-]`.
- **Keyboard first.** The terminal UI never needs a mouse; the window reaches everything by keyboard with a visible 2 px focus ring (`ember-bright` on dark, `ember-ink` on light).
- **Themes.** Dark (the default) and light are both first-class. High contrast follows the OS. `NO_COLOR` is honoured everywhere.
- **Localisation-ready.** Messages live in catalogues, there is no text in images (diagram labels are SVG text), and window layouts are right-to-left safe. English first; the voice is English-only, so other languages get plain until a voice is written for them.

## Contrast, as measured

| pair | ratio | use |
|---|---|---|
| `ember` on `night-900` | 6.75 | identity on dark |
| `ember-bright` on `night-900` | 8.36 | the running step |
| `ember-deep` on `night-900` | 5.32 | pressed, borders |
| `filter` on `night-900` | 7.92 | the carton, secondary accents |
| `ash-300` on `night-900` | 12.84 | body text on dark |
| `ash-400` on `night-900` | 7.45 | secondary text on dark |
| `ash-500` on `night-900` | 3.91 | disabled text or large type only |
| `ok` (dark) on `night-900` | 10.86 | success |
| `ember` on white | 2.80 | never text on light |
| `ember-deep` on white | 3.56 | large text only |
| `ember-ink` on white | 5.18 | ember as text on light |
| `ash-500` on white | 4.83 | secondary text on light |
| `ok` (light) on white | 5.02 | success on light |
| `night-900` on `ember` | 6.75 | button text on ember |

Buttons on `ember` use dark text. White on ember is 2.80:1 and is never used. The README badges' green `#2ea44f` is 3.21:1 on white, so it is for badges only.

## The terminal UI

- No spinners and no cursor tricks when stdout is not a terminal or `TERM=dumb`. Progress is then a line at 0%, then every 25%, then done, with no carriage returns.
- Under plain at a terminal (a screen reader), progress is a line at the start and one at the end.
- Prompts take one keypress, and the safe answer is the default and is said in the prompt. Without a terminal nothing is asked.
- `NO_COLOR`, `--no-color`, a non-terminal stdout and `TERM=dumb` give no escape codes at all, with the icons kept as text.
- Light terminals are detected (`theme = auto`) from `COLORFGBG`, then from an OSC 11 query bounded at 50 ms, and `theme = light` or `theme = dark` always wins.

## When plain is on

Plain is automatic under these conditions, and permanent with `voice = plain` in the installer's config:

| signal | how |
|---|---|
| `CI` set (not empty, `0` or `false`) | read from the environment |
| a screen reader | `CIG_SCREEN_READER=1` |
| `--plain` | flag |
| `--json` | structured output, no sentences |
| `voice = plain` | config |

### Finding: a screen reader cannot be detected from the environment

A terminal cannot say whether a screen reader is attached, and the variables that look like a signal are not one. `QT_ACCESSIBILITY=1` is set by default on KDE Plasma sessions: it is set on the maintainer's everyday machine with no screen reader running. Reading it would turn every KDE user's output plain. The same holds for the other desktop accessibility variables (`GNOME_ACCESSIBILITY`, `ACCESSIBILITY_ENABLED`), which some desktops set whether or not an assistive tool runs. The installer reads none of them. The test `plain_is_automatic_under_ci_a_screen_reader_and_the_voice_setting` asserts that `QT_ACCESSIBILITY=1` does not switch plain on.

What a screen-reader user does instead: set `voice = plain` once in `installer/config`, or export `CIG_SCREEN_READER=1` from the profile. A reliable cross-platform signal, if one appears, is a change to this table and a minor version of this standard.

## By surface

| surface | how it meets the bar |
|---|---|
| terminal | the rules above, tested by the golden screens in six variants |
| window (#18) | AccessKit; tokens from `generated/egui-theme.rs`; focus ring; reduced motion |
| docs | headings in order, text alternatives for diagrams, no screenshots of text (terminals are rendered as SVG) |
| diagrams | labels are SVG text; colours from `generated/svg-styles.css`; contrast as above |
