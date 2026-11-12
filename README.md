<!-- SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer) -->
<!-- SPDX-License-Identifier: CC-BY-4.0 -->
<p align="center">
  <img src="docs/assets/logo.svg" alt="cig-installer" width="600">
</p>

<p align="center">
  <b>Install, pin, update and remove the CigScript family, one carton at a time.</b><br>
  <i>Every build is checked before it runs, and every change is one step you can undo.</i>
</p>

<p align="center">
  <a href="https://github.com/otmof-ops/cig-script"><img alt="a CigScript tool" src="https://img.shields.io/badge/a-CigScript%20tool-f97316"></a>
  <a href="CHANGELOG.md"><img alt="status: pre-release" src="https://img.shields.io/badge/status-pre--release-f59e0b"></a>
  <img alt="built with Rust" src="https://img.shields.io/badge/built%20with-Rust-6b7280">
  <a href="LICENSE"><img alt="license FSL-1.1-ALv2" src="https://img.shields.io/badge/license-FSL--1.1--ALv2-6b7280"></a>
  <img alt="installs journaled" src="https://img.shields.io/badge/installs-journaled%20%26%20reversible-f97316">
  <a href="#no-telemetry"><img alt="telemetry: 0" src="https://img.shields.io/badge/telemetry-0-2ea44f"></a>
</p>

> Putting a tool on a machine is the one burn nobody journals. A script gets
> piped into a shell, files land somewhere, and when it goes wrong there is
> nothing to put back and no one to point at the cause.
>
> **cig-installer is the one that points.** It treats an install as a burn:
> the change is written down before it happens, it lands as one step, and it
> can be undone. It checks what it downloads, says which of the four kinds of
> no it hit when it refuses, and tells you what to type next.

`ciginstaller` (short: `cigi`) installs [`cig`](https://github.com/otmof-ops/cig-script),
[`cigo`](https://github.com/otmof-ops/cig-orchestrator),
[`cigcompile`](https://github.com/otmof-ops/cig-compile) and
[`cigar`](https://github.com/otmof-ops/cigar-script) at the versions a project
asks for. Everything comes out of a **carton**: one numbered set of builds
that were checked to work together. You can take the whole carton, a named
**pack** of it, or one **stick** from it, and whichever you take, it matches
everything else from the same carton.

Today, `ciginstaller` is built and tested on Linux. The first carton is
carton 0.1.0: `cig 1.21.0`, `cigo 1.19.0`, `CigCompile 0.4.0` and
`CigarScript 0.1.0`. Cartons stay at `0.N.P` until every product in them
has reached 1.0; that carton is 1.0.0. [Install](#install) says how to get the installer, and
[Limits, honestly](#limits-honestly) says what is missing.

## Sixty-second tour

The output below is in the shape of the project's output tests, against the
first carton.

```sh
ciginstaller install carton
```
```text
✓ carton 0.1.0 installed: cig 1.21.0, cigo 1.19.0, CigCompile 0.4.0, CigarScript 0.1.0
  trust: checksummed 4 (two sources agree); PATH block added to ~/.profile
  → open a new shell, or run: export PATH="$HOME/.cigscript/bin:$PATH"
```

The PATH block is shown before it is written, and `ciginstaller uninstall
--all` removes it again. Take less with `ciginstaller install pack dev` (cig
and cigo) or `ciginstaller install stick cig`.

```sh
ciginstaller which cigo
```
```text
✓ cigo 1.19.0, from carton 0.1.0 (pinned in .cig-carton)
  /home/u/.cigscript/toolchains/cigo/1.19.0/bin/cigo
```

A project says which carton it wants, and the shim in front of every tool picks
that build. `ciginstaller which cigo --why` prints each step it took to choose.

```sh
cd ~/work/site
ciginstaller pin carton 0.1
```
```text
✓ carton pinned to 0.1 in ~/work/site/.cig-carton
  no installed carton fits 0.1 yet; the first run fetches one (pin_fetch)
```

Here is a hypothetical later update, from carton 0.1.0 to 0.1.1:

```sh
ciginstaller update
```
```text
· carton 0.1.0 → 0.1.1: cig 1.21.0 → 1.21.1   (cigo, CigCompile, CigarScript unchanged)
  product        installed   carton 0.1.1   changes
  cig               1.21.0         1.21.1   programs refuse instead of turning in…
  cigo              1.19.0         1.19.0   —
  CigCompile         0.4.0          0.4.0   —
  CigarScript        0.1.0          0.1.0   —
✓ carton 0.1.1 installed; trial: 14 scripts dry-ran the same on the new cig
  → nothing to do. ciginstaller rollback puts 0.1.0 back.
```

`update` checks the manifest, chooses a carton, verifies it, runs the new `cig`
against your project's scripts in a trial, and only then switches. Change your
mind, and `ciginstaller rollback` puts the previous carton back. Run
`ciginstaller` with no arguments for the one-screen status:

```text
✓ carton 0.1.0: cig 1.21.0, cigo 1.19.0, CigCompile 0.4.0, CigarScript 0.1.0
  channel stable, trust checksummed 4
  waiting: carton 0.1.1 (cig 1.21.0 → 1.21.1)
  → ciginstaller update
```

When something refuses, it says why and what to type next, and
`ciginstaller explain <code>` has the long version. This one is real output:

```text
$ ciginstaller explain E1811
E1811 no such carton  (usage, installer family, since 0.1.0)

  kind of no: can't see any ciggies bro  (yours: you're pointed at the wrong place; wrong address: a path, a name, a directory not on PATH; ENOENT)

  The carton you named is not on this channel. The message says the newest that is, and where the one you asked for lives.

  what to type next: `ciginstaller carton list`

  known causes, ranked; doctor checks them in this order
  1. the version is mistyped
     probe: carton-list (the cartons in the cached manifest and the cartons installed under toolchains)
     remedy: `ciginstaller carton list` shows the real names
  2. the carton is on another channel
     probe: channel-list (the channels the cached manifest knows)
     remedy: `ciginstaller channel <name>`, then `ciginstaller carton list`
  3. the manifest in the cache is stale
     probe: manifest-cache (the cached manifest for the channel under installer/cache/manifests: its sequence, expiry and fetch time)
     remedy: `ciginstaller update --check` refetches it

  related: E1812, E1816
```

Every code is in [docs/ERRORS.md](docs/ERRORS.md).

## What you get

| what you get | how |
|---|---|
| one question: how much do you want | `install carton`, `install pack <name>` or `install pack --pick cig,cigcompile`, and `install stick <tool>`. Asked on a terminal when you give no target, and never asked with `--yes`, which suits CI and provisioning. The security preset (`chill`, `balanced`, `strict` or `paranoid`) is asked once, in the same place |
| everything from one carton | a pack is a named slice of a carton and a stick is one tool of it, so adding more later takes builds from the same carton and nothing mixes |
| builds checked before they run | three levels, and nothing below the floor installs: `pinned`, `attested` and `checksummed`. [SECURITY.md](SECURITY.md) says what each proves and what none of them do |
| an install is a burn | every change is a journaled transaction that ends in one atomic switch. A kill before the switch rolls back, and a kill after it rolls forward. No moment has a tool half-updated |
| the right build for each project | `pin carton <pin>` writes `.cig-carton`, `pin <tool> <version>` writes `.cig-version`, `.cigo-version` or `.cigar-version` for one tool, and `pin` also takes `--file` for a key in `cig-tasks.json` or `cigar.toml`. A `cigar = "1.0"` key in `cigar.toml` (or a `.cigar-version`) pins the CigarScript version, and the compiler comes with the cigar's carton: a project pinned to 1.0 builds with the cigcompile of the carton that holds cigar 1.0, after 1.1 is installed too, and a `carton` key in the same file wins. `which <tool> --why` shows every step of the choice, and `which --why` does it for every tool |
| updates that are tried first | `update` runs the new `cig` against your project before it switches, and shows the differences it found |
| an undo | `rollback` returns the previous carton, or one tool's build with `rollback <tool>` |
| combinations that fit | `check` is the marry-up matrix: it says when a tool needs a newer `cig`, or a pin and a carton disagree, before anything runs |
| a doctor and a clean exit | `doctor` says what it thinks and what it checked, `repair` restores files from the store or a fresh verified download, and `uninstall` removes one tool or everything the installer added, and nothing else |
| output you can script | `--json` prints one object per result, `--plain` the same facts in neutral words, and `NO_COLOR` is honored. Shapes are in [docs/JSON.md](docs/JSON.md) |
| errors that point | every refusal is one of the four kinds of no, with a stable code, the cause the doctor would check first, and the command to type next |
| no telemetry | [see the promise below](#no-telemetry) |

## Install

The `v0.1.0` release carries the Linux x86_64 and aarch64 tarballs, each with
a `.sha256` sidecar and a build attestation, and the filled bootstrap scripts
`install.sh` and `install.ps1`. The bootstrap script is the quick way;
building from source is the other. You need Rust 1.88 or newer to build.

```sh
git clone https://github.com/otmof-ops/cig-installer
cd cig-installer
cargo build --release
./target/release/ciginstaller --version
```

`cigi-shim` is built beside it, and `install` needs it there. Read an error
page, and run the suite, which installs fake products from a fixture release
source and so exercises installing, pinning, updating and rolling back:

```sh
./target/release/ciginstaller explain E1801
cargo test
```

`--home <dir>` (or `CIGSCRIPT_HOME`) points a run at a scratch home instead of
`~/.cigscript`, which is the safe way to try any command.

A manifest is assembled, never written by hand, from the products' own
releases: `cargo xtask manifest assemble`, described in
[manifest/README.md](manifest/README.md). `install --from <dir>` installs from a
mirror directory, and `install --from <file.tar>` from a bundle (see below). A
mirror directory and a bundle are each one source, so they are installed
`pinned` when this build has the digests compiled in and are otherwise refused
unless `allow_single_source` is set. Bundles are unsigned until
[#9](https://github.com/otmof-ops/cig-installer/issues/9).

For an image build or a first-boot welcome screen (CIG.OS,
[#22](https://github.com/otmof-ops/cig-installer/issues/22)),
`ciginstaller install --dry-run --json` lists the questions an install asks,
with their choices, and `--answers answers.json` (or `--preset` and
`--reports`) hands the answers in, so
`ciginstaller install --yes carton <c> --from bundle.tar --answers answers.json`
installs with no network and no question. Once answered, the questions are
not asked again.

### The bootstrap script

For a machine with nothing on it, each ciginstaller release carries
`install.sh` (Linux) and `install.ps1` (Windows). The `v0.1.0` release is
the first to carry them; the templates live in `packaging/bootstrap/` and
the release workflow fills them in with that release's tarball digests,
then attaches the result, with a `.sha256` and a build attestation for
each, to the release. Windows has no build yet, so `install.ps1`
stops and says so.

Download it, read it, then run it. Do not pipe it into a shell.

```sh
gh release download vX.Y.Z -R otmof-ops/cig-installer -p install.sh -p install.sh.sha256
sha256sum -c install.sh.sha256
gh attestation verify install.sh -R otmof-ops/cig-installer
less install.sh
sh install.sh --yes carton --preset strict    # arguments go to `ciginstaller install`
```

The script works out the platform, downloads that platform's tarball from
that exact release over HTTPS into a scratch directory, and checks its SHA-256
against the digest written into the script. If `gh` is on `PATH` it also runs
`gh attestation verify` on the tarball. Any mismatch stops it before anything
is unpacked, and nothing is left behind. Only then does it unpack under
`CIGSCRIPT_HOME` (default `~/.cigscript`) and run `ciginstaller install`,
passing along every argument you gave it. With none, `ciginstaller` asks its
install questions itself. It sends nothing about your machine, only plain
`GET`s of the release's tarball.

**This is integrity, not authorship.** The digests show that the bytes you got
are the bytes the release build made. The script and the tarballs are not
signed, so nothing shows who made them, and the script does not check a
signature. Signed metadata
([#9](https://github.com/otmof-ops/cig-installer/issues/9)) is not built. The
attestation, where `gh` can check it, ties the tarball to this repository's
release workflow and is the only provenance check there is: it runs when `gh` is
installed and signed in, must pass when it runs, and the script says when it could
not run it.

The platform packages are still planned
([#15](https://github.com/otmof-ops/cig-installer/issues/15)).

## Where things live

Everything is under `~/.cigscript` (or `CIGSCRIPT_HOME`):

| Path | What is in it |
| --- | --- |
| `bin/` | the shims: `cig`, `cigo`, `cigcompile`, `cigar`, and `ciginstaller` |
| `toolchains/<tool>/<version>/` | each installed build, read-only, made of hard links to the store |
| `humidor/` | cig's own shelf, `<version>/cig` and `index.json`, which cig reads |
| `humidor/store/` | the shared store: `objects/<aa>/<sha256>` by content, and `tmp/` |
| `installer/` | state, the shim index, the journal (`txn/`), caches, the lock |

A home made by an earlier build kept its objects in `store/`. The first command
that writes moves them under the humidor in one journaled step (`migrate store`),
and an interruption puts the home back as it was; a command that only reads
works on either layout.

## Commands

Every command takes `--json`, `--plain`, `--no-color`, `--yes`,
`--yes-irreversible`, `--quiet`, `--verbose`, `--wide`, `--offline` and
`--home <dir>`. A command that changes something
takes `--dry-run`, which prints the plan and writes nothing. `ciginstaller --help`
lists them all.

| command | does |
|---|---|
| `ciginstaller`, `status` | the status: the carton, what is in it, anything waiting, and the next step, in at most eight lines |
| `install carton`, `install pack <name>`, `install stick <tool>` | install from one carton, verified, as one transaction. Flags: `--carton`, `--channel`, `--no-path`, `--shell`, `--from <dir or file.tar>`, `--require`, `--allow-yanked`, `--preset` |
| `bundle --carton <v> [--targets a,b] --out <file.tar>` | one file with a carton's manifest and the builds of those targets (default: this machine's), each admitted before it goes in, for a machine with no network. Unsigned until #9. Install it with `install carton --from <file.tar>`; the same inputs make the same bytes |
| `mirror <dir> [--carton <v>] [--channel <c>] [--targets a,b]` | fill a directory with the channel's manifest and every build of the cartons (default: every carton that is not yanked), each admitted, laid out for `install --from <dir>` and `mirror = ...`. Run it again to bring it up to date; it removes nothing |
| `carton`, `carton list`, `carton show <v>`, `carton <v>` | which carton this machine is on, what exists, what is inside one, and moving to another |
| `channel [stable\|beta\|nightly]` | the channel this machine follows, or switch it. Nothing is installed |
| `use <carton>` | make an installed carton the machine's, with no download |
| `update` | check, choose, verify, trial and switch. `--check` only looks, `--to` names a carton, `--no-trial` skips the trial, `--auto` is what the daily job runs |
| `rollback [tool]` | back to the previous carton, or one tool's previous build |
| `pin carton <pin>`, `pin <tool> <version>` | write a project pin file: the carton, or one tool's (`cig`, `cigo`, `cigar`); a pin on `cigar` also selects `cigcompile` |
| `list`, `which [tool]`, `check` | what is installed, what runs here and why (every tool when none is named), and whether the combination fits |
| `doctor`, `repair` | find problems the installer owns, each with its fix command, and repair files from receipts |
| `uninstall <tool>`, `uninstall --all`, `remove <carton>` | remove a tool, everything the installer added, or one carton; your data stays |
| `gc` | remove cartons unused for `keep_days` and the store objects nothing uses |
| `shims sync`, `shims list` | rewrite the shims from the installer's state, and list them |
| `config list\|get\|set\|unset` | the installer's settings and where each value comes from, such as `channel`, `carton`, `trust`, `update_check`, `keep_days`, `bench_after_update`, `rollout`, `theme` and `voice`; a value the machine locks says `locked by <store>` ([organizations](docs/ORGANIZATIONS.md)) |
| `licences [tool] [--text]` | the license of the installer and of each build here (an SPDX id, or "not stated"), where its text is, and what was acknowledged at the first install; `--text` prints one license file; the field is the code license (docs are CC-BY-4.0); read-only, nothing is sent anywhere |
| `history` | the last installs, updates and rollbacks |
| `crash list\|show\|send\|delete` | every tool's crash reports, in one queue: `show` prints exactly what `send` sends, nothing leaves without `send`, and `reports = never` refuses |
| `crash status`, `crash test-route` | the queue, the `reports` setting and each route's last success and failure; one dry search per route, to see which can file a report |
| `bug [<product>]` | writes up what went wrong, asked in the voice with plain beside it (or from flags without a terminal), with the run's bundle, the version matrix, the carton, the OS and the doctor's findings, and shows exactly what would travel first; `--save` keeps it here, `--send` sends it through the crash route, `bug list` and `bug show <n>` are the reports saved here |
| `report [<run>]` | the version block and the bundle a bug report needs: `--out FILE` writes it, `--send` files it through the crash route, and `--block --json` is the block a product puts in its own report; otherwise nothing is written or sent |
| `programs [rebuild <name>]` | the compiled programs on this machine, found by their trailer on `PATH` and in the project's folders (never a scan of the disk), the cig each was built for, the cigcompile that made it and any advisory that covers that cig; `rebuild <name>` rebuilds one from its script with the carton's cigcompile on the installed cig, checks it with `cigcompile verify` and `cigcompile test`, and swaps it in as one transaction; nothing is sent anywhere |
| `bench` | measure this machine against the performance budgets (shim dispatch, check, switch, sizes, first lines), locally and read-only, with the median and p95 of each; nothing is sent anywhere |
| `self update` | update ciginstaller itself |
| `explain [code]` | what an error code means, why it comes and what fixes it |

Exit codes: 0 done, 1 failed, 2 refused (trust, a yanked carton, a mismatch,
a finding), 3 usage, 4 unreachable, 10 for `update --check --exit-code` when a
newer carton exists, 70 for a bug in the installer, and 127 when a shim cannot
start its tool. A shim otherwise passes the tool's own exit code through.

## Performance budgets

`ciginstaller bench` prints these on your machine, read-only, with the median
and p95 of each. The `bench` workflow measures them on a release build (a debug
build is slower and bigger, so its numbers are not judged) on every pull request
and every push to `main`. A row that a platform cannot measure yet says "not
measured" and is never counted as passing.

| what | budget | measured in CI |
|---|---|---|
| a shim dispatching to the right version | under 2 ms | Linux, macOS, Windows: `bench --strict` and `budget_20_neutral` |
| `ciginstaller check` (cached metadata) | under 200 ms | Linux, macOS: `bench --strict` against a fixture carton, and `budget_20`; Windows: not measured |
| a background update check (network) | under 1 s, at most daily | Linux, macOS: `budget_20` against a loopback server (the installer's own part, not a real network); Windows: not measured |
| installing `pack dev` | under 30 s on broadband | Linux, macOS: from a local mirror, so the installer's own part only (`bench --from`, `release_install`); Windows: not measured |
| installing a whole `carton` | under 60 s on broadband | Linux, macOS: from a local mirror (`bench --from`, `budget_20`); Windows: not measured |
| an atomic switch to a new carton | under 100 ms | Linux, macOS, Windows: `bench --strict` and `budget_20_neutral` |
| disk for five cigo versions | under 1.5x one version | Linux, macOS: `budget_20` on a real store; Windows: `budget_20_neutral`, from receipts |
| the first line of the status, and of any other command | under 50 ms, under 100 ms | Linux, macOS: `bench --strict` and `budget`; Windows: `bench --strict` |
| `ciginstaller` binary, shim binary | under 10 MB, under 500 KB | Linux, macOS, Windows: `bench --strict`; the static musl build is also judged by `ci/size-check.sh` |

The rows that run shell-script fixture products wait on the Windows install
fixture ([#21](https://github.com/otmof-ops/cig-installer/issues/21)). The tests
live in `crates/cigi-budgets` so that they can be built in release mode; run
them with `cargo build --release -p ciginstaller -p cigi-shim` and then
`cargo test --release -p cigi-budgets`.

## Limits, honestly

- **Linux first.** It is built and tested there. The bootstrap script ships
  with v0.1.0; `install.ps1` stops until a Windows build exists. macOS and
  Windows machines and platform packages are not built
  ([#5](https://github.com/otmof-ops/cig-installer/issues/5),
  [#15](https://github.com/otmof-ops/cig-installer/issues/15)).
- **Verified is integrity, not authorship.** There are no signing keys yet, so
  nothing is signed. The compiled-in pin list is empty today, so no build
  reaches `pinned`. Signed metadata, with protection against replayed or
  frozen releases, is [#9](https://github.com/otmof-ops/cig-installer/issues/9)
  and is not built. [SECURITY.md](SECURITY.md) has the detail.
- **The trial covers `cig`, `cigo` and `cigcompile`.** An update runs the new
  `cig` on your project's scripts, the new `cigo` (`cigo check`, then
  `cigo --dry-run run` of the `ci` chain and of each task your
  `cig-tasks.json` declares) and the new `cigcompile` (`cigcompile test` on
  your programs, built in a scratch folder), each beside the old build, and
  shows what differs before it switches. A tool the project does not use
  (no `cig-tasks.json`, no `.cig` programs) is skipped, and the trial says so.
- **Updates are checked when you run a command, and can run on a schedule you
  opt into.** `auto_update = notify` (the default) says once a day that a
  carton is out. `security` installs a carton that clears a security advisory
  in the signed manifest, and `all` installs every new carton. Both run
  `ciginstaller update --auto`: the same trial and the same rollback, so a
  carton that fails its trial is rolled back and the next terminal says so
  once. Setting either writes a daily job through the journal: a systemd user
  timer on Linux (`~/.config/systemd/user/cigi-update.timer`), a launchd agent
  on macOS and a Task Scheduler task on Windows. It runs at most daily, with
  a random delay of up to two hours, on mains power and when the machine is
  idle (systemd has no idle condition, so on Linux it runs at idle CPU and I/O
  priority). `auto_update = off` or `notify` takes it away, and so does
  `uninstall --all`. A value your administrator locks is the one obeyed.
- **An update is undone two ways, and only for as long as the transaction is
  kept.** `ciginstaller rollback` is the installer's own undo; cig can
  undo the same update for seven days (`cig runs adopt
  <home>/installer/txn/<id>`, then `cig unburn <run id>`). Past that window,
  the transaction is swept and rollback is the only undo.
- **Crash reports file through `gh` or your token, not yet the courier.**
  `ciginstaller crash list|show|send|delete|submit` work over cig's ashtray,
  and a crash in the installer lands there. `crash send` files a report in the
  repository of the product at fault, through `gh` with your login or the API
  with `CIG_GITHUB_TOKEN`, searching first so a known fault joins its issue.
  Without either it hands you a prefilled issue link or writes the report to
  a file (`--via link`, `--via file --out FILE`). `CIGI_ROUTES` can turn
  routes off. The relay that needs no login, a scoped token and the courier
  ([#24](https://github.com/otmof-ops/cig-installer/issues/24)) are still to
  come.
- **`ciginstaller report` is built; the products' stubs are not.** The version
  block (the installer, the carton, each tool's build, what the project pins,
  mismatches flagged) and the bundle behind a bug report work today, for an
  operation of the installer and, through cig, for a cig run. `cig report` and
  the other products' reports hand over to it when the family wiring lands.
- **`ciginstaller bug` is built as far as step 1 of
  [#29](https://github.com/otmof-ops/cig-installer/issues/29)**: the questions,
  the evidence for a run and the preview. The similarity search, "add mine",
  `bug status` and `bug withdraw`, the evidence for cigo, the guard, CigCompile
  and updates, and the products' own short ways in are not built.
- **`ciginstaller programs` is built without the signed index
  ([#30](https://github.com/otmof-ops/cig-installer/issues/30)).** Programs are
  found on `PATH` and in the project's folders (and `--project`), not by
  scanning the disk, and their advisories come from the cached manifest. The
  signed compatibility index and `which --why` wait for signing
  ([#9](https://github.com/otmof-ops/cig-installer/issues/9)). A program with no
  script on this machine is listed with its advisory so its author can be told,
  and cannot be rebuilt here.
- **Organization policy is built; one enforcer is not
  ([#17](https://github.com/otmof-ops/cig-installer/issues/17)).**
  `rollout = N%` stages a new carton to a share of machines, chosen by a hash
  of the machine id that never leaves the machine, and `status --json` is the
  inventory an organization's own tools collect. With no automatic install yet,
  the stage gates the update notice; your own `ciginstaller update` always
  takes the carton. A machine layer (`/etc/cigscript/config.toml`, a macOS
  configuration profile or the Windows registry) sets and locks a pinned
  carton, `auto_update`, `reports`, a mirror and the guard preset, as
  [docs/ORGANIZATIONS.md](docs/ORGANIZATIONS.md) describes. The guard preset is
  recorded and locked here; what it switches on is cigo's, and waits for
  [cig-orchestrator#66](https://github.com/otmof-ops/cig-orchestrator/issues/66).
- **Not built:** the window
  ([#18](https://github.com/otmof-ops/cig-installer/issues/18)).

The plan of record is the issues, starting with the architecture
([#1](https://github.com/otmof-ops/cig-installer/issues/1)) and its decision,
[docs/decisions/0001-architecture.md](docs/decisions/0001-architecture.md).

## No telemetry

This is a promise, and it is tested:

- ciginstaller sends nothing about you or your machine to anyone. No ids, no
  usage statistics, no install counts beyond what a download server's own logs
  show. A staged rollout (`rollout = N%`) is decided on your machine from a
  hash of its machine id with a fixed prefix; neither is ever printed, stored
  or sent.
- An update check is a plain `GET` of public metadata. It carries no
  identifier and no version in a query string (the comparison happens on your
  machine), only standard HTTP headers and `User-Agent: ciginstaller`.
  `update_check = off` stops even that, and `--offline` makes no request at
  all.
- A crash report leaves your machine only when you type `ciginstaller crash send`,
  and never when `reports` is `never` (`CIG_REPORTS=never` counts too).
  `crash show` prints exactly what would be sent: the fields of
  [schemas/report.schema.json](schemas/report.schema.json), with your paths as
  shapes and your user and host names removed wherever they are not part of
  a longer word or number (`sam-notes` loses `sam`, `samples` keeps it);
  `report --block` prints the version block exactly as a report carries it.
  Through `gh` or
  `CIG_GITHUB_TOKEN` the issue is filed under your own GitHub login, after a
  search that carries only the report's fingerprint and the repository;
  through `CIGI_REPORT_ROUTE` it is one plain `POST` with no query string,
  cookie, login or identifier.
- A report (`ciginstaller report <run> --send`) is the same: it goes only
  when you type `--send`, through the same route and the same `reports`
  setting, and it carries the version block, the operation's journal and the
  trial's differences with your paths as shapes and your names removed.
  `ciginstaller report <run>` alone writes and sends nothing; `--out FILE`
  writes it for you to read first.
- A bug report (`ciginstaller bug`) is yours the same way. It is previewed
  whole, section by section, before it is saved; `--save` and the last
  question only keep it here, and it is sent only by `--send` or "send it",
  through the same route and the same `reports` setting (`never` refuses).
  Your answers stay your words: a path, an address, a token, an e-mail address
  or your name in them is marked and asked about, a token is always replaced,
  and nothing is rewritten without being listed. `--yes` answers none of it.
- A test records every request an install makes against a local server and
  fails if any carries an identifier, a cookie or a query string. It covers
  `install` and the commands that read; it is being extended to `update`
  and `uninstall`
  ([#19](https://github.com/otmof-ops/cig-installer/issues/19)).

A way to make it report anything without your yes is a security bug, and
[SECURITY.md](SECURITY.md) says where to send it.

## Part of CigScript

cig-installer is the front door to the family: it installs
[CigScript](https://github.com/otmof-ops/cig-script) (`cig`),
[cig-orchestrator](https://github.com/otmof-ops/cig-orchestrator) (`cigo`) and
[cigcompile](https://github.com/otmof-ops/cig-compile), and its output
follows the same grammar they do: a headline, the detail, and the one thing to
do next. The family's other language is
[CigarScript](https://github.com/otmof-ops/cigar-script).

| you want | go here |
|---|---|
| to ask, or report something that went wrong | [SUPPORT.md](SUPPORT.md) |
| to report a security problem | [SECURITY.md](SECURITY.md), never a public issue |
| every error code | [docs/ERRORS.md](docs/ERRORS.md) |
| the `--json` shapes | [docs/JSON.md](docs/JSON.md) |
| why it is built this way | [docs/decisions/](docs/decisions/) |
| the next tag for every product | [docs/VERSIONS.md](docs/VERSIONS.md) |
| what changed | [CHANGELOG.md](CHANGELOG.md) |
| to send a fix | [CONTRIBUTING.md](CONTRIBUTING.md) |

## Licensing

ciginstaller is **source-available, not open source.** The code is under the
[Functional Source License, version 1.1, with an Apache-2.0 future license
(FSL-1.1-ALv2)](LICENSE). The Open Source Initiative does not count it as an
open source license, and this project does not call it one.

- **What you may do.** Use it, including inside a company for its own work;
  read it, study it, change it and share it with the license attached; and
  consult with it, meaning help others use it. The source is here for all of that.
- **What you may not do.** Make it available to others in a commercial product
  or service that substitutes for it or does the same job. Selling it, or
  selling access to it, is the clear case.
- **It becomes Apache-2.0.** Each version converts to
  [Apache-2.0](LICENSES/Apache-2.0.txt) two years after it is made available,
  and from then on that version carries Apache-2.0 terms.
- **Earlier commits.** The first 31 commits were published under Apache-2.0
  before this license took over. [LICENSING.md](LICENSING.md) lists which
  versions carry which license.
- **Documentation**: [CC BY 4.0](LICENSES/CC-BY-4.0.txt). Reuse and adapt
  with credit.
- **The name.** ciginstaller is a trademark of Jay Taylor, as CigScript is;
  [TRADEMARKS.md](TRADEMARKS.md) says what is free to do and what to ask about
  first.

This is a summary. The [LICENSE](LICENSE) file governs, and it is the text to
read before you rely on anything here. [REUSE.toml](REUSE.toml) maps every path
to its license. Contributions are accepted under the contributor agreement; see
[CONTRIBUTING.md](CONTRIBUTING.md). Security reports:
[SECURITY.md](SECURITY.md).
