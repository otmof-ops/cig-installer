<!-- SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer) -->
<!-- SPDX-License-Identifier: CC-BY-4.0 -->

# Components

Standard version 1.0.0. The shared pieces of the terminal UI (issue #32), built in `crates/cigi-ui`, with the window's equivalents named where they differ. Colour comes from roles in [tokens.json](tokens.json); nothing here names a raw colour.

## The grammar

Every message is three parts:

```text
<icon> <headline: what happened, one line>              always
  <detail: what was checked, what was found>            when there is any, indented 2
  → <the one thing to do next>                          whenever there is one
```

In voice mode the code is right-aligned on the headline line; in plain it follows in parentheses.

### Icons

| meaning | icon | ASCII | role | plain prefix |
|---|---|---|---|---|
| done | `✓` | `[ok]` | `ok` | `ok:` |
| running | `●` (the ember, breathing where motion is allowed) | `[..]` | `ember-bright` | `...` |
| ask | `?` | `[?]` | `warn` | `question:` |
| refused (one of the four kinds of no) | `✗` | `[x]` | by kind | `refused:` |
| a bug in us | `✗` | `[x]` | `danger` | `bug:` |
| notice | `·` | `[-]` | `ash-400` | `note:` |
| irreversible | `!` | `[!]` | `danger` | `irreversible:` |

ASCII icons are used when the locale is not UTF-8 (`LANG=C`), when `TERM` is `dumb`, and when the terminal is one whose font is known to lack the glyphs (`TERM` of `linux`, `vt100`, `vt102`, `vt220` or `ansi`: the font check). `CIG_ASCII=1` forces ASCII and `CIG_ASCII=0` forces the drawn icons. A terminal cannot report its font, so the font check is a list, not a probe.

### Colour roles

| role | dark | light | used for |
|---|---|---|---|
| `ok` | `#4ade80` | `#15803d` | done |
| `warn` | `#fbbf24` | `#b45309` | ask; refusals that are the door's |
| `danger` | `#f87171` | `#b91c1c` | bugs, irreversible |
| `info` | `#d1d5db` | `#6b7280` | refusals that are a pointer |
| `ember` | `#f97316` | `#c2410c` | arrows, changed cells |
| `ember-bright` | `#fb923c` | `#c2410c` | the step running now |
| `ember-ink` | `#f97316` | `#c2410c` | ember as text on light |
| `ash-400` | `#9ca3af` | `#6b7280` | headers, codes, secondary text |

Depth: truecolor uses the values; 256 colours use the nearest entry (`ember` is 208); 16 colours use `ok` green, `warn` yellow, `danger` red, `ember` bright yellow (basic ANSI has no orange) and `ash-400` dim. Light is chosen by `theme = light`, else `COLORFGBG`, else an OSC 11 query bounded at 50 ms, else dark; `theme = dark` or `light` always wins. `NO_COLOR`, `--no-color`, a non-terminal stdout and `TERM=dumb` give no escape codes.

## Progress

```text
● fetching carton 0.1.0  ━━━━━━━━━━━━━━━━━━╸━━━━━━━━  18.2/31.0 MB  2.1 MB/s  6 s
✓ verified: 4 builds, signatures and sha256s
```

One line per step; the running step's bar is in `ember`; done steps collapse to `✓`. Never more than one bar animating. Not a terminal: a line at 0%, then every 25%, then done, no carriage returns. Plain at a terminal: a line at the start and one at the end.

## Prompts

```text
? update to carton 0.1.1?  cig 1.20.0 → 1.20.1, cigo unchanged
  the trial ran clean. ciginstaller rollback undoes it.
  [Y]es  [n]o  [s]how changes                                   (enter = yes)
```

- The default is the safe answer; for anything irreversible it is no, and the prompt says so. An irreversible question leads with `!` in `danger`.
- On a terminal one keypress answers, with no enter. The terminal mode is set and restored by a small `sh` script that traps INT, TERM and HUP, so a Ctrl-C leaves the terminal as it was. If `sh`, `stty` or `dd` cannot be run, the prompt falls back to reading a line. Nothing here uses raw mode off a terminal.
- `--yes` answers the safe default, and `--yes-irreversible` is required for irreversible defaults.
- No prompt without a terminal: the safe default is taken, one line says so, and the exit code is 3 when that default was a refusal.
- Passwords and OS authentication never go through the UI's prompt; they go to the OS.

## Tables

```text
  product      installed   carton 0.1.1   changes
  cig          1.20.0      1.20.1         programs refuse instead of turning into cig
```

Left-aligned text, right-aligned numbers, headers in `ash-400`, changed cells in `ember`. Wrapped to the terminal width by cutting the last column with `…`; `--wide` (or `Table::wide`) shows it in full. `--json` carries the data.

## Boxes

A boxed section shows exactly what will travel (`crash show`, `bug`): a title on the top rule, a bar in front of every line, a closing rule. ASCII terminals get `+`, `-` and `|`.

## Errors

```text
✗ error[E1811 carton]: can't see any ciggies bro: there's no carton 1.4 on the stable channel
  = subject: 1.4
  = hint: newest on stable is 0.1.1; 0.2.0-beta.2 is on beta
  = explain: ciginstaller explain E1811
  = doctor: I think the channel is wrong, because the manifest lists no 1.4
  = fix: ciginstaller carton 0.1.1, or ciginstaller channel beta
```

The code and its area are always shown (`carton`, `check`, `fetch`, `trust`, `store`, `shim`, `pin`, `update`, `repair`, `usage`, `internal`). `= doctor:` is shown when the doctor has a finding (`InstallerError::with_doctor`), before `= fix:`. `--json` follows `diagnostic.schema.json`.

## Notices

At most one line at the end of any command, and never the same one twice in a day:

```text
· carton 0.1.2 is out: ciginstaller update
```

## Budgets

| measure | target | proved by |
|---|---|---|
| `ciginstaller` (status), first line | 50 ms | `first_line_budgets` (crates/ciginstaller/tests/budget.rs) |
| any command, first line | 100 ms | the same test |
| background query (OSC 11) | 50 ms, then the default | `detect::query_background`, tried on a pseudo-terminal with `examples/probe.rs` |
| lines of status output | at most 8 | the `status` golden |
| notices per command | at most 1 | `Ui::notice` |

Terminals that do not answer OSC 11 cost the whole 50 ms once per run, unless `theme` is set or `COLORFGBG` is; a terminal that answers the device-attributes request that follows the query ends the wait at once. The shim's slow path (`resolve`) never queries.

## The screens and their goldens

Every screen has a golden in six variants under `crates/cigi-ui/tests/golden/<screen>.<variant>.txt`, compared byte for byte by `golden_output_per_screen` (escape bytes are shown as `␛`). Regenerate with `UPDATE_GOLDEN=1 cargo test -p cigi-ui --test golden` and read the diff before committing it.

Variants: `dark` (truecolor, dark), `light` (truecolor, `COLORFGBG=0;15`), `ansi16`, `no-color` (`NO_COLOR`), `plain` (`--plain`, piped), `json` (`--json`, piped).

| screen | golden | what it shows |
|---|---|---|
| `ciginstaller install` | `install`, `install-questions` | the three questions (carton or stick, the security preset, crash reports), then the running step and the final `✓` with the one next step |
| `ciginstaller` (status) | `status` | the carton, the channel and trust, what is waiting, the next step, in at most 8 lines |
| `ciginstaller update` | `update`, `table`, `table-wide`, `prompt`, `progress`, `trial-differences` | the check, the table of changes, the trial with differences, the switch, the result, one notice |
| `ciginstaller which --why` | `which`, `which-why` | the resolver's table, then the answer |
| `ciginstaller doctor` | `doctor`, `finding` | findings grouped by product, each "I think this, because I checked that", ending with its fix command |
| `ciginstaller crash list` and `show` | `crash-list`, `crash-show` | the queue as a table; `show` prints exactly what travels, in boxes |
| `ciginstaller bug` | `bug` | one question per screen, the boxed preview, the send question, the result with the issue link |
| `ciginstaller carton list` | `carton-list` | cartons as a table with what changed |
| `ciginstaller programs` | `programs` | compiled programs on the machine, their cig, any advisory |
| errors | `error-no-such-carton`, `error-bug` | a refusal painted by its kind; a bug in `danger` |
| irreversible prompt | `prompt-irreversible` | the `!` lead, default no, "can't be undone" |

Screens whose commands are still to be built (`crash`, #28; `bug`, #29; `programs`, #30) are drawn with the shared components from fixtures, so their look is fixed before their commands land. The `doctor` screen is drawn the same way; the command's own output is tested in `crates/ciginstaller/tests/a4_doctor.rs`.

### Golden changes in this standard version

The regenerated goldens differ from the earlier ones only in these ways, each checked by reading the diff:

| change | goldens affected |
|---|---|
| `ok` is `#4ade80` (dark) and `#15803d` (light) instead of the placeholder greens; `warn`, `danger` and the light ember likewise | every colour variant that paints a `✓`, `?`, `!` or arrow |
| ember is `ember-ink` (`#c2410c`) on light, never `#ea580c` | `*.light.txt` with an arrow, a changed cell or a bar |
| a refusal is painted by its kind, so `E1811` is `info` and not red | `error-no-such-carton.*` colour variants |
| the error label is `error[E1811 carton]` instead of `error[E1811 installer]`, and `E1890` is `error[E1890 internal]` | `error-*`, all variants |
| plain names the kind of no (`not found:`, `not allowed:`) | `error-no-such-carton.plain`, `finding.plain` |
| the product is CigCompile in prose | `install`, `status`, `update`, `table*` (the product column), `carton-list`, `programs`, `doctor`, `install-questions` |
| an irreversible prompt leads with `!` and says `this can't be undone` in the voice | `prompt-irreversible.*` |
| the JSON bodies are unchanged | all `json` variants of existing screens |
| new screens | `bug`, `carton-list`, `crash-list`, `crash-show`, `doctor`, `install-questions`, `programs`, `trial-differences`, `which-why` |
