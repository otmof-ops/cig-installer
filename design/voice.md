<!-- SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer) -->
<!-- SPDX-License-Identifier: CC-BY-4.0 -->

# Voice, and plain beside it

Standard version 1.0.0. Every user-facing message has two forms, written together in the product's catalogue (`crates/cigi-ui/src/catalogue.rs` for the installer): neither is derived from the other.

- **The voice** is warm, Australian and a bit cheeky, never cruel.
- **Plain** says the same facts in neutral words. It is automatic under `CI`, a screen reader (`CIG_SCREEN_READER`), `--plain` and `--json` (structured), and permanent with `voice = plain` in config. See [accessibility.md](accessibility.md) for why a screen reader is not guessed.

## Structure, the same in both

1. One headline line that says what happened.
2. The detail: what was checked, what was found.
3. The one thing to do next, as a command or a button.

## Rules

- Name the thing: "cigo 1.12.0 needs cig 1.15", not "a version mismatch".
- Never blame the person: "that path isn't there", not "you typed it wrong".
- Say what is irreversible.
- The voice never jokes about loss, security or money. There it goes straight.
- No exclamation marks in errors, and none in the catalogue (`the_voice_keeps_its_rules`).
- No "simply", "just" or "easy".
- Smoking words are product nouns (burn, ash, ashtray, stick, pack, carton, lighter) with the manual's name beside them in docs. They are not jokes sprinkled on top. Plain uses the manual's names.
- Plain keeps every fact the voice carries (`plain_has_the_same_facts`).

## The four kinds of no

| kind | voice | plain |
|---|---|---|
| `cant-see-any` | can't see any ciggies bro | not found |
| `these-are-mine` | these are MY ciggies | not allowed |
| `dont-have-any` | don't have any over here | none available |
| `never-heard-of-you` | never heard of you | not authenticated |

The voice forms are the family's catchphrases and equal `hear` in `errors/registry.toml` (`the_four_kinds_of_no_match_the_registry`). They are used for a refusal only; a bug has no catchphrase, and the voice goes straight.

## Fifty paired examples

Each row is the same message in both forms. The first group is the catalogue as built; the rest are the shape every product's messages take.

### From the catalogue (cigi-ui)

| # | voice | plain |
|---|---|---|
| 1 | `nobody's at the keyboard to ask: took "no" for "remove everything?" (pass --yes to say yes for next time)` | `no terminal to ask on: took "no" for "remove everything?" (pass --yes to say yes)` |
| 2 | `--yes: said yes to "update to carton 0.1.1?"` | `--yes: yes to "update to carton 0.1.1?"` |
| 3 | `this one can't be undone, so --yes alone won't do it: pass --yes-irreversible` | `this step is irreversible: pass --yes-irreversible to say yes` |
| 4 | `(enter = yes)` | `(enter = yes)` |
| 5 | `(enter = no; this can't be undone)` | `(enter = no; this cannot be undone)` |
| 6 | `that's not one of them. pick one of: y n` | `not an option. pick one of: y n` |
| 7 | `can't see any ciggies bro: there's no carton 1.4 on stable` | `not found: there's no carton 1.4 on stable` |
| 8 | `these are MY ciggies: cigo 1.4.1 needs cig 1.8.0 or newer` | `not allowed: cigo 1.4.1 needs cig 1.8.0 or newer` |
| 9 | `don't have any over here: carton 0.1.0 is not installed` | `none available: carton 0.1.0 is not installed` |
| 10 | `never heard of you: the signature on carton 0.1.0 does not match` | `not authenticated: the signature on carton 0.1.0 does not match` |

### Products' real messages

| # | voice | plain |
|---|---|---|
| 11 | `can't see any ciggies bro: ~/work/tidy.cig isn't there` | `not found: ~/work/tidy.cig isn't there` |
| 12 | `carton 0.1.1 installed: cig 1.20.1, cigo 1.17.0, CigCompile 0.2.0` | `carton 0.1.1 installed: cig 1.20.1, cigo 1.17.0, CigCompile 0.2.0` |
| 13 | `trial: 14 scripts dry-ran the same on the new cig` | `trial: 14 scripts planned the same on the new cig` |
| 14 | `nothing to do. ciginstaller rollback puts 0.1.0 back.` | `nothing to do. ciginstaller rollback restores 0.1.0.` |
| 15 | `I think the PATH block is missing from your profile` | `the PATH block is missing from your profile` |
| 16 | `because I checked ~/.profile, ~/.bashrc and ~/.zshrc: none has the block` | `checked ~/.profile, ~/.bashrc and ~/.zshrc: none has the block` |
| 17 | `I think a program was built on an older cig` | `a program was built on an older cig` |
| 18 | `the new cig plans 1 script differently` | `the new cig plans 1 script differently` |
| 19 | `keep 0.1.0` | `keep 0.1.0 (the safe answer)` |
| 20 | `carton 0.1.2 is out: ciginstaller update` | `note: carton 0.1.2 is available: ciginstaller update` |
| 21 | `fetching carton 0.1.0` | `fetching carton 0.1.0` |
| 22 | `verified: 4 builds, signatures and sha256s` | `verified: 4 builds, signatures and sha256 sums` |
| 23 | `home directory is not yours to write: ~/.cigscript` | `home directory unsafe or unwritable: ~/.cigscript` |
| 24 | `the disk is full: 31 MB needed, 12 MB free` | `disk full: 31 MB needed, 12 MB free` |
| 25 | `another ciginstaller is working here (pid 4121); waiting` | `another ciginstaller holds the lock (pid 4121); waiting` |
| 26 | `there's no cig 1.15 in this carton; the nearest is 1.14.2` | `no cig 1.15 in this carton; the nearest is 1.14.2` |
| 27 | `the pin says carton 0.2 and it's been pulled` | `the pinned carton 0.2 was yanked` |
| 28 | `couldn't fetch carton 0.1.1: the server said 503` | `download failed for carton 0.1.1: HTTP 503` |
| 29 | `the manifest is a day past its use-by` | `the manifest expired a day ago` |
| 30 | `the manifest went backwards (sequence 12, had 14); not trusting it` | `manifest sequence went backwards (12, had 14); refused` |

### The shape, by kind of message

| # | voice | plain |
|---|---|---|
| 31 | `done: 3 tools installed` | `ok: 3 tools installed` |
| 32 | `nothing changed` | `no changes made` |
| 33 | `1 finding, 1 fix to run` | `1 finding, 1 fix to run` |
| 34 | `stopped at the trial; nothing was changed` | `stopped at the trial; no files changed` |
| 35 | `crash report c-0007 is waiting for you to read it` | `crash report c-0007 is waiting for review` |
| 36 | `send crash reports? only after you have read them` | `send crash reports? only after review` |
| 37 | `filed: https://github.com/otmof-ops/cig-installer/issues/41` | `filed: https://github.com/otmof-ops/cig-installer/issues/41` |
| 38 | `2 programs; 1 needs a rebuild` | `2 programs; 1 needs a rebuild` |
| 39 | `cigo 1.17.0, from carton 0.1.0 (pinned in .cig-carton)` | `cigo 1.17.0, from carton 0.1.0 (pinned in .cig-carton)` |
| 40 | `no pin here, so the machine's carton answers` | `no pin in this directory; using the machine carton` |
| 41 | `the carton is only partly installed: 2 of 3 tools` | `carton partly installed: 2 of 3 tools` |
| 42 | `the shim for cigo has been shadowed by /usr/local/bin/cigo` | `the cigo shim is shadowed by /usr/local/bin/cigo` |
| 43 | `a profile block was edited by hand, so it was left alone` | `a profile block was edited and was left unchanged` |
| 44 | `the self-update failed and the old binary is back` | `self-update failed; the previous binary was restored` |
| 45 | `rolled back to 0.1.0` | `rolled back to 0.1.0` |
| 46 | `needs your password: that goes to the system, not to us` | `needs OS authentication: this is handled by the system, not by ciginstaller` |
| 47 | `no terminal to answer on, so the safe answer was taken` | `no terminal; the safe default was taken` |
| 48 | `offline: this step needs the network` | `offline requested; this step needs the network` |
| 49 | `ciginstaller 0.1.0 is up to date` | `ciginstaller 0.1.0 is up to date` |
| 50 | `a bug in ciginstaller: the shim index has two machine lines` | `bug: the shim index has two machine lines` |

Rows 31 to 50 are the shape each product follows when it writes its own catalogue; the products' sweep (#34) moves their real messages into these forms. Row 46 is the one place the voice says plainly where the work goes: it never jokes about money or security. Row 50 has no catchphrase, because a bug in us is not one of the four kinds of no.

## Writing a new message

1. Write the plain form first as a check that the facts are all there, then write the voice form beside it, in the same edit.
2. Give both the same placeholders (`{0}`, `{1}`).
3. Add it to `catalogue::ALL` so `every_message_has_both_forms` and `the_voice_keeps_its_rules` cover it.
4. If it is a refusal, find its kind in `errors/registry.toml`; the colour follows the kind.
