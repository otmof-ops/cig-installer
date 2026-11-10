<!-- SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer) -->
<!-- SPDX-License-Identifier: CC-BY-4.0 -->

# Accessibility

Standard version 1.0.0. The bar, and how each surface meets it.

## The bar

- **Text contrast** is at least 4.5:1 for body text and at least 3:1 for large text and UI components, in both themes. The test `every_token_pair_used_meets_contrast` (crates/cigi-ui/tests/contrast.rs) checks every pair listed under `pairs` in [tokens.json](tokens.json), and `every_terminal_role_is_text_that_reads_on_its_theme` checks every terminal role against both backgrounds.
- **Never color alone.** Every state has an icon and a word. The icons are `✓ ● ? ✗ ! ·` and, in ASCII, `[ok] [..] [?] [x] [!] [-]`.
- **Keyboard first.** The terminal UI never needs a mouse; the window reaches everything by keyboard with a visible 2 px focus ring (`ember-bright` on dark, `ember-ink` on light).
- **Themes.** Dark (the default) and light are both first-class. High contrast follows the OS. `NO_COLOR` is honored everywhere.
- **Localization-ready.** Messages live in catalogues, there is no text in images (diagram labels are SVG text), and window layouts are right-to-left safe. English first; the voice is English-only, so a non-English locale gets plain until a voice is written for it (see below).

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

- No spinners and no cursor tricks when stderr is not a terminal or `TERM=dumb`. Progress is written to stderr, so it is stderr that decides: it is then a line at 0%, then every 25%, then done, with no carriage returns.
- Under plain at a terminal (a screen reader), progress is a line at the start and one at the end.
- Prompts take one keypress in the voice at a terminal, and the safe answer is the default and is said in the prompt. Without a terminal nothing is asked: the safe default is taken, one line says so, and the exit code is 3 when that default was a refusal.
- Under plain (and for anything irreversible) the answer is a typed line. An irreversible question takes a typed `yes` and Enter, and the default is no; `y` alone does not confirm it.
- Whatever was typed before a question is drawn is thrown away, and so is whatever follows the key for about 100 ms (an arrow key's tail, autorepeat, an Enter pressed from habit), so nothing typed for one question answers the next. A byte that starts an escape sequence is never an answer.
- An answer that is not an option names the valid keys and asks again. After three, the run stops without changing anything and exits 3. A default is never taken silently.
- `NO_COLOR`, `--no-color` and `TERM=dumb` give no escape codes at all, with the icons kept as text. So does any stream that is not a terminal: stdout and stderr are judged separately, so `ciginstaller list | less` has no escapes in the list and still colors its errors at the terminal.
- Light terminals are detected (`theme = auto`) from `COLORFGBG`, then from an OSC 11 query bounded at 50 ms, and `theme = light` or `theme = dark` always wins.
- When the background is not found, no word is colored: text stays at the terminal's own foreground, and only non-text glyphs (the icons, the arrows, the bars) are colored, in a neutral palette in which every color is at least 3:1 on both black and white.
- In 16 colors each theme has its own codes, chosen from the standard xterm palette so that text reaches 4.5:1 (`every_ansi16_role_reads_on_the_standard_xterm_palette_in_both_themes`). Basic ANSI has no orange, so ember is bright yellow on dark and bright blue on light.
- The background query is off under plain, a screen reader, `voice = plain` and a non-English locale; over SSH (`SSH_CONNECTION`, `SSH_TTY`), where a slow link would deliver the reply late; and when the process is not in the terminal's foreground group (`/proc/self/stat`, `tpgid` against `pgrp`). A terminal that does not answer costs the 50 ms once per run; setting `theme` skips the query.
- The terminal helpers (`sh`, `stty`, `dd`, `od`, `wc`, `kill`) are run by absolute path from `/bin` or `/usr/bin` with an empty environment but `TERM`, so a hostile `PATH` is never searched. The scripts restore the terminal on INT, TERM and HUP, and a drop guard restores it on a panic.

## When plain is on

Plain is automatic under these conditions, and permanent with `voice = plain` in the installer's config:

| signal | how |
|---|---|
| `CI` set (not empty, `0` or `false`) | read from the environment |
| a non-English locale (`LC_ALL`, `LC_MESSAGES`, then `LANG`; `C`, `POSIX` and `en` are English) | the voice is English-only, so other languages get plain until a voice is written for them |
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
| diagrams | labels are SVG text; colors from `generated/svg-styles.css`; contrast as above |
