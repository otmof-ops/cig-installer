<!-- SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer) -->
<!-- SPDX-License-Identifier: CC-BY-4.0 -->
# Changelog

All notable changes to cig-installer. The format follows Keep a Changelog; the
project follows semantic versioning. Nothing is released yet, so everything
below is under Unreleased, and the first version will take it as its section.

## Unreleased

Installing, pinning, switching, updating and rolling back the family, on
Linux, with every build checked first and every change one undoable step.

### Crash reports (#10, #28 part 1)

- **`ciginstaller crash list|show|send|delete|submit`** over cig's own ashtray
  (`<home>/ashtray`, the same files and fields), so every tool's crash is in
  one queue. `show` prints exactly what `send` sends; `send` honors `reports`
  (`never` refuses, also as `CIG_REPORTS=never`) and carries no identifier.
  With no issues route yet (the relay and token of #28, the courier of #24)
  the report stays queued and the output says so. E1857.
- **The installer's own crash** is written to the ashtray in cig's schema with
  the origin `ciginstaller`; nothing is sent.
- **`schemas/report.schema.json`** is the allowlist for every product, and the
  family fingerprint (kind, location, message, product, major version) never
  includes a path, user name or host. The family's own paths stay readable in
  titles and bodies.

### Reports (#11, #3)

- **`ciginstaller report [<run>]`** puts together what a bug report needs, and
  writes or sends nothing until you say so (`--out FILE`, `--send`).
  - **The version block**: the installer, the carton and channel, each tool's
    build (its version, the digest of what was installed, how it was
    trusted), what the project pins (as versions and file names, never
    paths), the OS and kernel, the metadata versions, and whether the
    combination is inside the manifest's guarantees. A combination no carton
    holds is flagged by name, as in "cigo 1.18.0 with cig 1.20.0: not from the
    same carton", and so is a build whose `requires` the running builds do
    not meet or that the manifest yanks.
  - **The bundle**: the block, the operation's journal (redacted), the trial's
    differences (`update` now keeps them with its transaction) and the
    metadata versions it saw. For a cig run, cig's own bundle through
    `cig report-bundle <run> --json`, or `cig --json report <run>` for a cig
    that does not have it yet.
  - **`--block --json`** is the machine form of the block that a product
    embeds in its own report.
  - **`--send`** goes through the crash route and the `reports` setting
    (`never` refuses), as a report of kind `report`, with no identifier.
  - It works when `state.json` is the broken thing: the block says so.
  - E1858 when the bundle cannot be built or written.

### Tripwires (#28, the installer's row)

- **After an update or a rollback the installer reads its work back from
  disk**: the shim-index against `state.json`, every shim against the carton's
  build, and after a rollback every toolchain against its receipt. A mismatch
  keeps the work (nothing is undone and the exit stays 0), prints
  `warning E1849` with the next step (`ciginstaller repair`, then
  `ciginstaller crash show <report>`), and saves a report of kind `tripwire`
  in the ashtray: high severity after a rollback, medium after an update.
  Nothing is sent.

### Release prune and the archive index (#26)

- **`cargo xtask release prune --dry-run`** lists, per product, which
  releases stay (the three newest, and every version a carton or a `--pin`
  names, with who pins it) and which would move to the archive, with a reason
  for each. Moving them (`--yes`) waits for the signed index (#9) and refuses
  for now.
- **`archive/index.json`** (format 1), read by `cig-compat`: where each
  archived version's tag and assets went, with their digests. A manifest names
  it with `[archive] index = "https://..."`.
- **Downloads fall back to the archive**: when a build's release is gone from
  the product's repository, the installer reads the index the manifest names
  and downloads the archived copy, still at the digest the manifest pins. An
  index that lists other bytes for a build is refused (E1825), and the index
  is only read when the release is gone.

### Licenses (#27)

- **`ciginstaller licences [tool] [--text] [--json]`** lists the installer and
  every installed build with its license (an SPDX id, or "not stated"), the
  SPDX page, where the license file is, and what was acknowledged. `--text` prints
  one license file. It reads this machine only and sends nothing.
- **A per-build `license` field** (an SPDX expression, optional) in the
  manifest's `[product.*]` tables, read by `cig-compat`, filled by
  `cargo xtask manifest assemble` from `manifest/products.toml` (which says
  so when a build has none), and recorded in each receipt at install. A
  manifest or receipt without it still works and shows "not stated".
- **The first-install display of the licenses**: shown once, and again only
  when a license new to the machine comes in. The licenses are copyright
  permissions, so the notice is acknowledged, not accepted: the answer
  (carton, the installer license's sha256, time, how) is kept in `state.json`
  and sent nowhere. `--yes` records `acknowledged-by-flag`; with no terminal
  the licenses are noted in the report, not confirmed; a no, Ctrl-C or end of
  input at a terminal installs nothing (E1876). E1877 and E1878 are the
  `licences` errors.

### Bootstrap script

- **`install.sh` and `install.ps1` templates** (`packaging/bootstrap/`) and
  `ci/fill-bootstrap.sh`, which writes a release's tarball digests into them.
  The script downloads its platform's tarball from that release over HTTPS,
  checks the SHA-256 (and `gh attestation verify` where `gh` exists), refuses
  on any mismatch with nothing left behind, unpacks under `CIGSCRIPT_HOME` and
  runs `ciginstaller install` with the arguments it was given. Integrity, not
  authorship: nothing is signed until #9. The release workflow attaches the
  filled scripts, with checksums and attestations, to the draft release. There
  is no installer release yet. `install.ps1` stops until a Windows build exists.
- `ci/test-bootstrap.sh` runs the filled script against a local fixture in CI.

### License

- **The code is under the Functional Source License, version 1.1, with an
  Apache-2.0 future license (FSL-1.1-ALv2)** from the licensing commit on
  (#27). Each version converts to Apache-2.0 on the second anniversary of the
  date it is made available. FSL is source-available, not open source.
- The first 31 commits were published under Apache-2.0 before this change, are
  tagged `apache-2.0-last`, and stay under it; copies taken then keep that
  grant. [LICENSING.md](LICENSING.md) lists which versions carry which license.
- Documentation is CC-BY-4.0. [TRADEMARKS.md](TRADEMARKS.md), `NOTICE`,
  `THIRD-PARTY-NOTICES.md` and `REUSE.toml` say the rest, and contributions
  come in under the contributor agreement.

### Offline: bundle, mirror and install from a file (#16)

- **`bundle --carton <v> [--targets a,b] --out <file.tar>`** writes one file
  holding the manifest and its checksum, every build of the carton for those
  targets with its checksum, and a small index. Only builds admitted at the
  usual levels go in, and the bytes are checked again as they are written. The
  same inputs give the same file.
- **`install --from <file.tar>`** unpacks the bundle into a private directory in
  the home, installs from it with no network, and removes it. A bundle is one
  source, so it is `pinned` or refused exactly as a mirror directory is, and
  nothing lowers the floor. Bundles are unsigned until #9, and the output says
  so. The doctor names installs that came from one source, a bundle by name.
- **`mirror <dir>`** fills a directory with the manifest and every build, each
  admitted, laid out for `install --from <dir>` and `mirror = ...`. Running it
  again only adds, repairs a damaged file, and removes nothing. The manifest
  goes in last, and a manifest older than the one already there is refused.
- New codes: E1837 (a bundle that cannot be unpacked), E1838 (a mirror
  directory that is refused), E1839 (a bundle that cannot be written).

### Documentation

- A README in the family's order, `SECURITY.md` that says what is verified and
  what is not, `SUPPORT.md`, and this changelog.
- `docs/ERRORS.md`, generated from `errors/registry.toml` by
  `cargo xtask errors-doc`, with a test that fails when it is stale.
- A logo and a social preview in the family's style.

### The foundation (#36)

- **The carton manifest** and the checks that say whether a combination fits
  (the marry-up matrix, #4): a product that needs a newer `cig`, a stick
  outside its carton, a yanked build, an advisory.
- **The home.** One directory (`~/.cigscript`, or `--home`, or
  `CIGSCRIPT_HOME`), the installer's own content-addressed store, receipts
  for every installed file, and a lock.
- **Transactions.** Every change is journaled and ends in one atomic switch
  of the shim index. A kill before the switch rolls back and a kill after it
  rolls forward, and the next run recovers either.
- **Verified downloads.** One HTTPS client with no identifiers, a manifest
  admitted before anything it names, and three levels for a build: `pinned`,
  `attested` where `gh` can verify, and `checksummed` where two sources agree.
- **The shim** that runs the version a project pins, with a median of 0.24 ms
  from start to `exec` on the shim's own clock.
- **Output.** One grammar (a headline, the detail, the next step) in the
  voice and in `plain`, truecolor with 256 and 16-color fallbacks, `NO_COLOR`,
  and `--json` with one object per result ([docs/JSON.md](docs/JSON.md)).
- **Errors.** A registry of codes `E1800` to `E1899`, each with its kind of
  no, its ranked causes and the command to type next, and
  `ciginstaller explain <code>`.
- **No telemetry** as a tested property (#19): the HTTP client and the
  fixture server check that requests carry no identifier, no cookie and no
  query string.

### Install, pin and resolve (#37, #38)

- **`install carton`, `install pack <name>`, `install pack --pick a,b` and
  `install stick <tool>`**, each from one carton, as one transaction, with
  the PATH block shown before it is written (`--no-path` skips it).
- **`carton`, `carton list`, `carton show`, `channel`, `use`, `list`.**
- **`pin carton <pin>`, `pin cig <pin>` and `which <tool> [--why]`.** A
  project pins a carton in `.cig-carton`, or a key in `cig-tasks.json` or
  `cigar.toml`; the resolver says which step chose the build.
- **`cargo xtask manifest assemble` and `cargo xtask pins`**, which assemble
  the next manifest from the products' own releases and write the compiled-in
  pins.

### Update, roll back and tidy (#39)

- **`update`:** check, choose, verify, trial and switch, in that order. The
  trial runs the new `cig` against the project's scripts and shows what it
  plans differently. `--check` only looks, and `--exit-code` exits 10 when a
  newer carton exists.
- **`rollback [tool]`** to the previous carton, or one tool's previous build.
- **`self update`**, **`gc`** and **`check`**.
- **The release workflow:** a `v*` tag builds ciginstaller and its shim for
  Linux x86_64 and aarch64, with a checksum and a build attestation for each,
  and creates a draft release. Publishing it is a human act.

### Bench and the budgets (#35, #20, #19)

- **`bench`** measures the installer's own budgets on this machine, locally and
  read-only, and compares them with `bench/bench.toml`: shim dispatch, `check`,
  the switch (in a scratch home), the sizes, the first lines, and, with
  `--from <mirror>`, the installs. Each shows its median and p95. `--json` has
  the reserved shape, extended. `--strict` fails with `E1866` when one is over.
  The family view, cartons against cartons, `--watch` and the window's Bench view
  are not built: they wait on otmof-ops/cig-script#179, #30, #33 and #29, and
  say so with `E1867`.
- **`bench_after_update`** (off by default): after an update, a short local sample
  against the last one, with a regression shown once. Nothing is sent anywhere.
- **Budget tests** for `check`, the switch, a whole carton, five cigo versions on
  disk, the update check and the binary sizes; the no-telemetry test now covers
  `update`, `doctor --network`, `repair`, `uninstall` and `self update --check`.
