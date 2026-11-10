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
[`cigo`](https://github.com/otmof-ops/cig-orchestrator) and
[`cigcompile`](https://github.com/otmof-ops/cig-compile) at the versions a
project asks for. Everything comes out of a **carton**: one numbered set of
builds that were checked to work together. You can take the whole carton, a
named **pack** of it, or one **stick** from it, and whichever you take, it
matches everything else from the same carton. CigarScript joins the carton
when it has a release.

Today, `ciginstaller` is built and tested on Linux, and no carton has been
published yet. [Install](#install) says what you can do now, and
[Limits, honestly](#limits-honestly) says what is missing.

## Sixty-second tour

The output below comes from the project's output tests. The carton numbers
(`0.1.0`, `0.1.1`) are stand-ins until the first carton is published.

```sh
ciginstaller install carton
```
```text
✓ carton 0.1.0 installed: cig 1.20.0, cigo 1.17.0, CigCompile 0.2.0
  trust: pinned 1, checksummed 2 (two sources agree); PATH block added to ~/.profile
  → open a new shell, or run: export PATH="$HOME/.cigscript/bin:$PATH"
```

The PATH block is shown before it is written, and `ciginstaller uninstall
--all` removes it again. Take less with `ciginstaller install pack dev` (cig
and cigo) or `ciginstaller install stick cig`.

```sh
ciginstaller which cigo
```
```text
✓ cigo 1.17.0, from carton 0.1.0 (pinned in .cig-carton)
  /home/u/.cigscript/toolchains/cigo/1.17.0/bin/cigo
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

```sh
ciginstaller update
```
```text
· carton 0.1.0 → 0.1.1: cig 1.20.0 → 1.20.1   (cigo, CigCompile unchanged)
  product      installed   carton 0.1.1   changes
  cig             1.20.0         1.20.1   programs refuse instead of turning in…
  cigo            1.17.0         1.17.0   —
  CigCompile       0.2.0          0.2.0   —
✓ carton 0.1.1 installed; trial: 14 scripts dry-ran the same on the new cig
  → nothing to do. ciginstaller rollback puts 0.1.0 back.
```

`update` checks the manifest, chooses a carton, verifies it, runs the new `cig`
against your project's scripts in a trial, and only then switches. Change your
mind, and `ciginstaller rollback` puts the previous carton back. Run
`ciginstaller` with no arguments for the one-screen status:

```text
✓ carton 0.1.0: cig 1.20.0, cigo 1.17.0, CigCompile 0.2.0
  channel stable, trust pinned 1, checksummed 2
  waiting: carton 0.1.1 (cig 1.20.0 → 1.20.1)
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
| the right build for each project | `pin carton <pin>` writes `.cig-carton`, `pin <tool> <version>` writes `.cig-version`, `.cigo-version` or `.cigar-version` for one tool, and `pin` also takes `--file` for a key in `cig-tasks.json` or `cigar.toml`. `which <tool> --why` shows every step of the choice, and `which --why` does it for every tool |
| updates that are tried first | `update` runs the new `cig` against your project before it switches, and shows the differences it found |
| an undo | `rollback` returns the previous carton, or one tool's build with `rollback <tool>` |
| combinations that fit | `check` is the marry-up matrix: it says when a tool needs a newer `cig`, or a pin and a carton disagree, before anything runs |
| a doctor and a clean exit | `doctor` says what it thinks and what it checked, `repair` restores files from the store or a fresh verified download, and `uninstall` removes one tool or everything the installer added, and nothing else |
| output you can script | `--json` prints one object per result, `--plain` the same facts in neutral words, and `NO_COLOR` is honored. Shapes are in [docs/JSON.md](docs/JSON.md) |
| errors that point | every refusal is one of the four kinds of no, with a stable code, the cause the doctor would check first, and the command to type next |
| no telemetry | [see the promise below](#no-telemetry) |

## Install

`ciginstaller` has no release yet, and no carton has been published, so there
is nothing to download. What you can do today is build it from source. You
need Rust 1.88 or newer.

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

Running `install` against the real source ends where it should, because the
`manifest` release that names the first carton does not exist yet:

```text
$ ciginstaller install carton
✗ error[E1820 fetch]: https://github.com/otmof-ops/cig-installer/releases/download/manifest/manifest-stable.toml does not exist (404)
  = explain: ciginstaller explain E1820
  = fix: `ciginstaller install carton`, and add `--offline` once a copy is cached
```

The exit code is 4. Nothing was installed. `--home <dir>` (or `CIGSCRIPT_HOME`)
points a run at a scratch home instead of `~/.cigscript`, which is the safe way
to try any command.

A manifest is assembled, never written by hand, from the products' own
releases: `cargo xtask manifest assemble`, described in
[manifest/README.md](manifest/README.md). `install --from <dir>` installs from a
mirror directory, and `install --from <file.tar>` from a bundle (see below). A
mirror directory and a bundle are each one source, so they are installed
`pinned` when this build has the digests compiled in and are otherwise refused
unless `allow_single_source` is set. Bundles are unsigned until
[#9](https://github.com/otmof-ops/cig-installer/issues/9).

### The bootstrap script

For a machine with nothing on it, each ciginstaller release will carry
`install.sh` (Linux) and `install.ps1` (Windows). **There is no installer
release yet, so there is no script to download today**; what is in this
repository are the templates (`packaging/bootstrap/`), and the release
workflow fills them in with that release's tarball digests, then attaches the
result, with a `.sha256` and a build attestation for each, to the draft
release. Windows has no build yet, so `install.ps1` stops and says so.

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
| `update` | check, choose, verify, trial and switch. `--check` only looks, `--to` names a carton, `--no-trial` skips the trial |
| `rollback [tool]` | back to the previous carton, or one tool's previous build |
| `pin carton <pin>`, `pin <tool> <version>` | write a project pin file: the carton, or one tool's (`cig`, `cigo`, `cigar`) |
| `list`, `which [tool]`, `check` | what is installed, what runs here and why (every tool when none is named), and whether the combination fits |
| `doctor`, `repair` | find problems the installer owns, each with its fix command, and repair files from receipts |
| `uninstall <tool>`, `uninstall --all`, `remove <carton>` | remove a tool, everything the installer added, or one carton; your data stays |
| `gc` | remove cartons unused for `keep_days` and the store objects nothing uses |
| `shims sync`, `shims list` | rewrite the shims from the installer's state, and list them |
| `config list\|get\|set\|unset` | the installer's settings and where each value comes from, such as `channel`, `trust`, `update_check`, `keep_days`, `theme` and `voice` |
| `history` | the last installs, updates and rollbacks |
| `self update` | update ciginstaller itself |
| `explain [code]` | what an error code means, why it comes and what fixes it |

Exit codes: 0 done, 1 failed, 2 refused (trust, a yanked carton, a mismatch,
a finding), 3 usage, 4 unreachable, 10 for `update --check --exit-code` when a
newer carton exists, 70 for a bug in the installer, and 127 when a shim cannot
start its tool. A shim otherwise passes the tool's own exit code through.

## Limits, honestly

- **Nothing installs from the network yet.** No installer release and no
  carton manifest are published. Build from source, and expect `install` to
  stop at `E1820` until the first carton is out.
- **Linux first.** It is built and tested there. The bootstrap script exists
  as a template and has no release to ship with yet; `install.ps1` stops until
  a Windows build exists. macOS and Windows machines and platform packages are
  not built
  ([#5](https://github.com/otmof-ops/cig-installer/issues/5),
  [#15](https://github.com/otmof-ops/cig-installer/issues/15)).
- **Verified is integrity, not authorship.** There are no signing keys yet, so
  nothing is signed. The compiled-in pin list is empty today, so no build
  reaches `pinned`. Signed metadata, with protection against replayed or
  frozen releases, is [#9](https://github.com/otmof-ops/cig-installer/issues/9)
  and is not built. [SECURITY.md](SECURITY.md) has the detail.
- **The trial covers `cig`.** An update runs the new `cig` on your project's
  scripts. Trials of `cigo` and `cigcompile` wait for entry points in those
  tools.
- **Updates are checked when you run a command.** There is no background
  service. `auto_update` accepts `security` and `all`, and both behave as
  `notify`.
- **`cig unburn` cannot undo an update yet.** `ciginstaller rollback` is the
  undo.
- **The first carton holds `cig`, `cigo` and `cigcompile`.** CigarScript joins
  it when it has a release.
- **Not built:** crash reports and bug filing
  ([#10](https://github.com/otmof-ops/cig-installer/issues/10),
  [#29](https://github.com/otmof-ops/cig-installer/issues/29)), organization
  policy ([#17](https://github.com/otmof-ops/cig-installer/issues/17)), and
  the window ([#18](https://github.com/otmof-ops/cig-installer/issues/18)).

The plan of record is the issues, starting with the architecture
([#1](https://github.com/otmof-ops/cig-installer/issues/1)) and its decision,
[docs/decisions/0001-architecture.md](docs/decisions/0001-architecture.md).

## No telemetry

This is a promise, and it is tested:

- ciginstaller sends nothing about you or your machine to anyone. No ids, no
  usage statistics, no install counts beyond what a download server's own logs
  show.
- An update check is a plain `GET` of public metadata. It carries no
  identifier and no version in a query string (the comparison happens on your
  machine), only standard HTTP headers and `User-Agent: ciginstaller`.
  `update_check = off` stops even that, and `--offline` makes no request at
  all.
- Crash reports are not built yet. When they are, they leave your machine
  only with your consent, under the allowlist `cig` already keeps.
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
