<!-- SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer) -->
<!-- SPDX-License-Identifier: CC-BY-4.0 -->

# 0003: The family runs through the installer

- Status: accepted
- Date: 2026-11-10
- Issues: otmof-ops/cig-installer#1 to #35

## Context

cig, cigo, cigcompile and cigar each grew their own way to install, update,
uninstall, check themselves and report a bug. cig and cigo carry their own
download, verify and self-replace code, two install scripts each, and a
`doctor` that checks PATH and packages. cigcompile and cigar document a manual
curl, checksum and copy recipe. Four products meant four routes, and none of
them could see the others: a cigo guard could outlive cigo, a cig update could
not tell a person that a bug they reported was fixed, and a doctor could not
say that one product's build did not suit another's.

ciginstaller already owns the carton, the manifest, the store, the shims and the
transactions. The people who use the family expect one command for the whole of it.

## Decision

Everything a person does to install, update, remove, diagnose or report a
product goes through `ciginstaller`. The products keep what only they can do and
what must work without the installer.

### What every product does

1. **Install.** Release assets keep their names, `.sha256` sidecars and
   attestations: the manifest reads them. A product's `install.sh` and
   `install.ps1` become redirect stubs for one release, printing
   `<tool> is installed by ciginstaller now: ciginstaller install stick <tool>`
   and exiting 1. Building from a checkout stays, documented in CONTRIBUTING.
2. **Update.** cig's and cigo's `update` become stubs for `ciginstaller update`
   (`--check` maps to `--check`; a product-version `--to` is explained, since
   updates come by carton). Their own download, verify and self-replace code is
   removed; version parsing and the release listing stay.
3. **Uninstall.** `cig uninstall` and `cigo uninstall` are stubs for
   `ciginstaller uninstall <tool>`.
4. **Doctor.** Each product has one hidden, read-only, non-recursive
   `<tool> __doctor --json` that never calls `ciginstaller`. State fixes live behind
   `<tool> __doctor --fix`; fixes to PATH or packages are removed. `cig doctor <run>`
   stays. The shape is the installer's own finding shape:
   `{"tool","version","findings":[{"check","severity":"ok|note|action","found","because","fix"}]}`.
5. **Reports.** Crash capture and local queueing stay in each product. The
   user-facing `crash`, `report` and `bug` commands become stubs for the
   installer's, once it has them (F3). Bundle builders stay as internal entries.
6. **Stubs behave the same everywhere.** Find the installer on `PATH`, then
   `$CIGSCRIPT_HOME/bin`, then `~/.cigscript/bin`; run it with the mapped arguments,
   inherit stdio, pass the exit code through; when it is missing, say what it
   does and how to get it, and exit 127.

### What the installer does (this change)

- **`doctor --tools`** runs `<tool> __doctor --json` for each installed product, reading the JSON and not the exit code (cig's always exits 0)
  and shows its findings as its own, under the tool's heading, with the same
  severities, causes and fixes. A tool's `action` counts toward the exit code.
  It never runs `<tool> doctor`, which for cig and cigo is a stub that comes back
  here. A build without `__doctor` (an unknown subcommand, or an answer that is not
  JSON) gets a note: "this <tool> build has no self-check; it needs a newer build".
  The existing bounds stay: a timeout, output caps, a process-group kill, no
  library-loading variables, the home as the working directory, and no run of a
  toolchain that differs from its receipt. JSON of the wrong shape is E1879;
  a tool that cannot be run is E1864 as before.
- **cigo's guard.** `uninstall cigo` and `uninstall --all` first run
  `cigo guard uninstall --yes`, when the guard is installed (its `guard/state.json`
  or any shim in `shims/` under `CIGO_HOME`, default `~/.cig-orchestrator`). A
  failure stops the uninstall with nothing removed (E1829), because shims left on
  PATH would call a cigo that is gone.
- **No update-check cache.** cig has no run banner that reads `update-check.json`; only its old doctor did, and that check is the installer's now. The installer writes no such file.
- **`cig crash announce-fixed`** runs from the new cig after an update that
  changed cig. It never fails the update; a cig without the subcommand is
  skipped without a word.
- **`cigo __lower --json`** (cig-orchestrator#142) runs through the shim
  `<home>/bin/cigo` after a transaction that changed what the `cig` or
  `cigo` shims start (`install`, `update`, `rollback`, `use`; `repair` and
  `self update`, which link the shims again), when a cigo is installed.
  cigo's launcher asks cig for its bytecode VM only when a module of this
  cigo is kept for this cig, and it tells the two apart by what it sees of
  them on a call, so the step runs the way the person's next `cigo` will
  start: through the shim, with the home's `bin/` first on PATH as for
  `cigo guard uninstall`. The working directory is a new empty temporary
  folder, the person's alone and removed afterwards, because cigo's script
  takes the working directory as its pack and the home must never be that.
  The transaction is committed first, so the step cannot fail it: a cigo
  from before `__lower` is silent, and a timeout, a crash or an answer that
  is not cigo's document is one line. The time limit is 120 s
  (`CIGI_LOWER_TIMEOUT_SECS` shortens it for tests). The installer prints
  what cigo answered and carries its document in `--json` output as
  `cigo_lower`.

## Consequences

- One route in and out for every product, with one place to look when something is wrong.
- The products lose their update and install code. Their release assets are an
  interface and may not be renamed.
- `doctor --tools` depends on the `__doctor --json` shape. A product that changes
  the shape without the installer gets E1879, not a crash.
- Releases of the products are separate steps, each after the license gate.

## What each product changes

| Phase | cig | cigo | cigcompile | cigar |
|---|---|---|---|---|
| F1 | `__doctor`; `update`, `uninstall`, `doctor` stubs; `crash announce-fixed`; install scripts become stubs | `__doctor`; `update`, `uninstall`, `doctor` stubs; install scripts become stubs; guard uninstall stays as the hook | none | none |
| F2 | none | none | docs rewritten to the installer; `__doctor` | docs; `__doctor` as a self-check; E1104 message; bug template; tag-only release workflow |
| F3 | `crash`, `report`, `bug` stubs | `crash`, `report` stubs | bug route named | E1104 names `ciginstaller bug cigar` |
