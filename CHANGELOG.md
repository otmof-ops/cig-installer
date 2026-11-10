<!-- SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer) -->
<!-- SPDX-License-Identifier: CC-BY-4.0 -->
# Changelog

All notable changes to cig-installer. The format follows Keep a Changelog; the
project follows semantic versioning. Nothing is released yet, so everything
below is under Unreleased, and the first version will take it as its section.

## Unreleased

Installing, pinning, switching, updating and rolling back the family, on
Linux, with every build checked first and every change one undoable step.

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
