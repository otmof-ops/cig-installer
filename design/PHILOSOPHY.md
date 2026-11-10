<!-- SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer) -->
<!-- SPDX-License-Identifier: CC-BY-4.0 -->

# The design standard: philosophy

Standard version 1.0.0. ciginstaller is the front door to the CigScript family, so it sets the standard every product, document and screen follows (issue #31). Nothing here is a new direction: it is the family's look and voice as the logos, diagrams, `terminal.svg`, the README badges, the lexicon, the four kinds of no and the doctor already have them, written down so it can be checked.

The machine-readable source is [tokens.json](tokens.json). A product declares the version of this standard it meets, and the standards sweep (#34) checks it.

## The eight principles

| principle | where it comes from | what it means for design |
|---|---|---|
| Nothing real happens unless you burn. | CigScript's one rule | Every action that changes something says so before it happens, shows what will change, and says how to undo it. Nothing destructive hides behind a default. |
| Be the one that points. | CigScript's README | Every failure says where, why and the one thing to type or click next. No dead ends. |
| Four kinds of no. | the lexicon, the error registry | Every refusal is one of `cant-see-any`, `these-are-mine`, `dont-have-any` or `never-heard-of-you`, and looks like its kind everywhere. |
| "I think this, because I checked that." | the doctor | Diagnoses show their evidence, and ask before fixing. |
| The voice, and plain beside it. | every message in cig and cigo | The voice is warm, Australian, a bit cheeky, never cruel. Plain gives the same facts in neutral words. Both are written together. |
| Honest limits. | cigo's "Limits, honestly" | Screens and docs say what something cannot do as plainly as what it can. |
| No ads, no nags, no telemetry. | ciginstaller#19 | Nothing asks twice, nothing sells, nothing leaves the machine without a yes. |
| Calm. | the logo's smoke, drawn as calm curves | Motion is slow and small. Urgency comes from words and position, never from flashing. |

## Each principle, done right and wrong

### Nothing real happens unless you burn

Right, from `ciginstaller update`:

```text
? update to carton 0.1.1?  cig 1.20.0 → 1.20.1, cigo unchanged
  the trial ran clean. ciginstaller rollback undoes it.
  [Y]es  [n]o  [s]how changes                                   (enter = yes)
```

It says what changes, offers the changes, and names the undo.

Wrong: `Updating... done.` The person never saw what moved, and cannot tell how to go back.

For an irreversible step the default is no, the prompt says so, and `--yes` alone does not say yes:

```text
! remove everything ciginstaller added?  toolchains, store, shims, profile blocks
  [y]es  [N]o                                 (enter = no; this can't be undone)
```

### Be the one that points

Right:

```text
✗ error[E1811 carton]: can't see any ciggies bro: there's no carton 1.4 on stable
  = hint: newest on stable is 0.1.1; 0.2.0-beta.2 is on beta
  = fix: ciginstaller carton 0.1.1, or ciginstaller channel beta
```

Wrong: `error: an error occurred`. It names nothing, locates nothing and offers no next step.

### Four kinds of no

A refusal is painted by its kind, not always red:

| kind | whose | role | plain name |
|---|---|---|---|
| `cant-see-any` | yours: you are pointed at the wrong place | `info`, with a "where" pointer | not found |
| `these-are-mine` | the door: it knows you and said no | `warn` (a lock icon in the window) | not allowed |
| `dont-have-any` | nobody's: right place, nothing there | `info` (an empty icon) | none available |
| `never-heard-of-you` | the door: it does not know you | `warn` (a key icon) | not authenticated |
| a bug in us (`E1890`, `E1891`) | ours | `danger` | bug |

Right: a missing carton reads as a pointer, not an alarm. Wrong: every refusal in red, so the person cannot tell "you typed it wrong" from "we have a bug".

### "I think this, because I checked that"

Right:

```text
· I think the PATH block is missing from your profile                      E1845
  because I checked ~/.profile, ~/.bashrc and ~/.zshrc: none has the block
  → ciginstaller repair
```

Wrong: `Your PATH is broken. Fixing...`. No evidence, and it fixed before asking.

### The voice, and plain beside it

Right: both forms exist and say the same facts.

| voice | plain |
|---|---|
| `can't see any ciggies bro: ~/work/tidy.cig isn't there` | `not found: ~/work/tidy.cig isn't there` |

Wrong: a voice line with no plain form, or a plain form that drops a fact the voice carries.

### Honest limits

Right: a doc ends with "Limits, honestly" and says what the tool cannot do. Wrong: a feature list with no limits section.

### No ads, no nags, no telemetry

Right: at most one notice per command, never the same one twice in a day, and crash reports are shown in full before a yes. Wrong: a "rate us" line, a second notice, or a report sent in the background.

### Calm

Right: the ember breathes (a 2.4 second ease-in-out glow) on the step running now, and nothing else moves. Wrong: a spinner on every line, a flashing error, or color change as the only signal. `prefers-reduced-motion` and `plain` turn even the breathing off.
