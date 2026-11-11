<!-- SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer) -->
<!-- SPDX-License-Identifier: CC-BY-4.0 -->

# Lexicon

The family words ciginstaller uses, in the installer's sense. Some of them are also words in
CigScript, where they mean something else; the second table says which sense is meant where, so
nobody mixes them up. Smoking words are product nouns. In plain mode (`--plain`) and in the
docs, the manual's name is given beside them.

## In the installer

| word | means | the manual's name |
|---|---|---|
| **stick** | one product's build, taken from a carton: `ciginstaller install stick cig`. | a single tool |
| **pack** | a named slice of a carton, a few products that go together: `dev` (cig, cigo), `builder` (dev plus CigCompile and CigarScript), `security` (dev plus the guard), or your own with `--pick`. Packs are data in the carton manifest, so any carton can add one. | a set of tools |
| **carton** | the whole family at versions that were tested together, released whenever any product releases. Everything the installer installs comes out of one carton, so it all matches. | a release set |
| **channel** | the line of cartons you follow: `stable`, `beta` or `nightly`. | release channel |
| **burn** | one change to the machine, made through the journal: it says what will change before it happens, and a failed or interrupted one rolls back to exactly the previous state. Every install and update is a burn. "Nothing real happens unless you burn." | a transaction |
| **unburn** | undo a burn: `cig unburn <run>`. | roll back a transaction |
| **humidor** | the shelf under `~/.cigscript/humidor/`: cig's build mirror (`<ver>/cig` and `index.json`, which cig itself reads) and, since ADR 0001 was accepted, the installer's shared store in `humidor/store/`. The installer keeps the mirror in step with the cig it installs. | cig's build mirror |
| **shim** | the tiny dispatcher in `~/.cigscript/bin/` (`cig`, `cigo`, ...) that finds the build the project's carton pins, or the machine's carton, and runs it. | launcher |
| **pin** | a project's choice of carton (`carton 1.1` in `.cig-carton`) or of one tool's version (`ciginstaller pin cigo 1.3`). Old per-tool pins still work. | version constraint |
| **preset** | how careful the guard is: `chill`, `balanced`, `strict` or `paranoid`. Asked once at install and recorded in the `preset` setting. | security profile |
| **ashtray** | where the family's crash reports and leftovers are kept until you send or clear them. | crash report queue |

## Where the words also mean something in CigScript

| word | in the installer | in CigScript code |
|---|---|---|
| **stick** | one product's build | a constant |
| **pack** | a named set of products | a closure, and a scope of chains |
| **burn** | a journaled change to the machine | running a script for real (as opposed to a dry run) |

In CigScript source, `stick` and `pack` keep their language meanings. In the installer, in its
output and in its documentation, they mean the installer's senses above.

## The four kinds of no

Every refusal is one of `cant-see-any`, `these-are-mine`, `dont-have-any` or
`never-heard-of-you`, and looks like its kind everywhere. The installer's codes are
`E1800` to `E1899`.
