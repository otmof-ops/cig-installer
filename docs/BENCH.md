<!-- SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer) -->
<!-- SPDX-License-Identifier: CC-BY-4.0 -->
# Measuring the installer

Two things are called a benchmark here, and they are different files.

| file | read by | what it holds |
|---|---|---|
| `bench/budgets.toml` | `ciginstaller bench`, which embeds it | the budgets of #20 and #32 (a shim dispatch in 2 ms, `check` in 200 ms, an atomic switch in 100 ms, the binaries' sizes) and what a run is compared with |
| `bench/bench.toml` | `cig bench` (otmof-ops/cig-script#179) | the installer's suite in the family tool's format, so `cig bench --family` reads it next to cig's, cigo's and cigcompile's |

## Where a time is judged

A budget is judged on a release build, in `crates/cigi-budgets` (its `Cargo.toml` says why it is
a package of its own):

```sh
cargo build --release --locked -p ciginstaller -p cigi-shim
cargo test --release --locked -p cigi-budgets
```

The functional suite (`cargo test -p ciginstaller`) runs a debug build on whatever machine runs
it, so it checks what a command does and leaves, never how long it took: a slow or busy machine is
not a failing install. `install pack dev` from a local mirror is judged in
`cigi-budgets/tests/release_install.rs` (3 s for the installer's own part of #20's 30 s); on a
debug build that test prints its time and does not judge it.

The terminal-driven tests (`a4_install_ux`, `a3_update`, `a5_licences` and others, through
`script(1)`) have a cap so none waits forever on a prompt: 30 seconds. Most of their time is the
install after the last answer, which a busy machine slows several times over. On such a machine,
`CIGI_TEST_PTY_SCALE=N` (a whole number from 1 to 20) multiplies the cap; unset, it is 30 seconds.
A key typed before a question is ready to read is thrown away by the question, so the harness types
a chunk again while the child has printed nothing but its echo, the next question has not appeared
and the output has been quiet for a second; each test prints how long it took and how often it
typed (`--nocapture`), and a test that hits the cap says which chunk it was waiting on.

## `cig bench` on the installer

```sh
cargo build --release -p ciginstaller && cig bench
cig bench --timings          where each case's time went
```

The suite is the start-up group of #32: `--version`, `status` and `list` on a home with nothing
installed. Each case runs in a scratch home, so the cartons, pins and settings of whoever runs it are
not read, and each is checked against its expected output before it gets a time. `bench/bench.toml`
names what it does not hold yet, because each needs an installed carton, a mirror or another
product's build that a scratch home does not have: cigo's hook on each cig of a carton, a compiled
program against `cig run`, `update --check` and `which --why`, an update's trial on a 50-script
project, the window's timings, and a carton's install from a local mirror.

## The installer's phases

With `CIG_TIMINGS_FD` set (`cig bench --timings` sets it), the installer writes its phases to that
descriptor as timings lines (format 1, `docs/BENCH.md` in cig-script), and only then:

| phase | what |
|---|---|
| `start` | the first line of `main` to the parsed command line, the crash hook included |
| `home` | reading the installer's state |
| `command` | the command itself |
| `output` | printing the answer |

What is left of the wall time (exec and exit) is `other` in the report. `--version` and `--help`
leave before the first phase ends, so they write none. The variable is removed from the installer's
own environment once read, so a tool it runs does not write the same phases again. A value that is
not a descriptor of 3 or more, or one that is not open, is ignored. The code is `crates/ciginstaller/src/timings.rs`.

## What `ciginstaller bench` does with the family flags

`--watch`, `--carton`, `--with`, `--matrix`, `--product` and `--only` still say E1867: wrapping
`cig bench --family` for them, a Bench view in the window and the opt-in check after an update wait
on a released `cig bench --family` and on the carton resolver (#30). `cig bench --family --product
ciginstaller=.` does the family view today.

## CI

`.github/workflows/bench-nightly.yml` calls cig-script's reusable `bench.yml` on a schedule and appends
each run to `history/ciginstaller.jsonl` on the `bench-history` branch.
