<!-- SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer) -->
<!-- SPDX-License-Identifier: CC-BY-4.0 -->

# 0005: A family tool belongs to the home it lives in

- Status: accepted
- Date: 2026-11-12

## Context

`CIGSCRIPT_HOME` has two jobs. It is cig's state root, where its runs, tin and keys live. It also chose the family home: ciginstaller and the shims read it to find the settings, the toolchains and the shims.

cig's box moves the first job. A boxed hop, and so every cigo task on Linux, gets `CIGSCRIPT_HOME` set to the run's inbox (`<home>/inbox/<run>/home`). That is the one place a guarded hop may write, and the state root for a cig started inside it. The installer read the same variable, so inside a task it saw an empty family: no settings and no toolchains. cigo's `cigarscript` setup stage ran `ciginstaller install carton --yes` there. It fetched the whole carton again into the inbox, or was refused as a single source. The carton gate found this (`cigo-task-sees-the-family-home`, `products-together-polyglot`).

## Decision

1. **The family home is where the running tool lives.** A family binary started from a home's `bin/` (a shim, the installer's own copy) or living under that home's `toolchains/` belongs to that home, whatever `CIGSCRIPT_HOME` says. A folder is a home when it holds `installer/`.
2. **`CIGSCRIPT_HOME` chooses the home only for a binary that lives in no home.** That covers a development build, the bootstrap installer and a test's binary. The order is: `--home`, then the home the binary lives in, then `CIGSCRIPT_HOME`, then `~/.cigscript`.
3. **`CIGSCRIPT_HOME` stays cig's state root.** The shim does not overwrite it for the program it starts. It sets the variable only when it is unset, as before, so cig inside a box keeps the inbox the box gave it. The installer's settings, toolchains and shims do not move with it.
4. **Nothing installs from inside a task.** A command that changes the home stops with `E1905` when the home cannot be written where it runs, before it takes the lock and with nothing written. The commands that only read (`status`, `config get`, `check`, `which`, `list`) answer for the home the tool lives in.

The rule is written once, in `cigi-shim`'s `home_containing`. The shim uses it through `choose_home`, and the installer uses it through `cigi_home::Home::for_running` (the context, the global flags, the crash hook and `crash`).

## The option set aside

cig could hand the outer home down to a boxed hop in a variable of its own, so the installer inside reads that variable. That was set aside for three reasons:

- it needs a cig release, so the cig 1.21.0 already published would stay broken;
- the family home would become something a task's environment can name, which is what a box is there to stop;
- this rule needs no change in cig, so it holds for every cig released.

## What changes for a person

- **A shim of one home with `CIGSCRIPT_HOME` naming another:** the shim runs its own home's build. It used to run the other home's. `CIGI_SHIM_VERBOSE=1` still names both homes, and now says the home the shim lives in wins.
- **Using another home's tools:** put that home's `bin/` on `PATH`, or run its installer with `--home <dir>`.
- **A home's own `bin/ciginstaller` with `CIGSCRIPT_HOME` naming another folder:** it works on the home it sits in. `ciginstaller doctor` names the two homes only when `--home` points elsewhere, or when the binary sits in a `bin/` that is not a home.
- **A development build or a test's binary:** follows `CIGSCRIPT_HOME` as before.
