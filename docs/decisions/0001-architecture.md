<!-- SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer) -->
<!-- SPDX-License-Identifier: CC-BY-4.0 -->

# 0001: Architecture of ciginstaller

- Status: proposed
- Date: 2026-11-10
- Issue: otmof-ops/cig-installer#1
- Companion: `SPEC.md` (the build contract; every path, format and signature lives there)

## Context

The family is four products that each install and update themselves a different way.

- **cig** ships one binary per platform (`cig-v<ver>-<os>-<arch>[.exe]`) with a `.sha256` beside it. `cig update` downloads it with `curl` or `gh`, checks the checksum, and replaces the running file with a rename. A second path, the humidor (`~/.cigscript/humidor/<ver>/cig` plus `index.json`), keeps older cigs so a script that says `# cig 1.1` runs on a cig that matches.
- **cigo** (cig-orchestrator) ships a tarball (`cig-orchestrator-v<ver>.tar.gz`: a shell launcher, `orchestrate.cig`, docs). It needs a cig on `PATH` at or above a floor it checks itself. `cigo update` replaces the files where they sit.
- **cigcompile** fetches cig runtimes for other targets and accepts only digests compiled into itself (`src/digests.rs`, written by `ci/digests.sh`). It keeps them under `~/.cigscript/cigcompile/runtimes/`.
- **cigar** (CigarScript) has no release yet.

Nothing tells a person which cigo goes with which cig, nothing lets two projects use two builds of the same tool, and each updater has its own idea of "verified". Issue #1 asks for one binary that installs, pins, updates, repairs and removes the family, the way rustup and mise do for their ecosystems.

Facts about the existing code that bound the design:

1. The `cigscript` crate exposes almost everything as `pub mod` (`burn`, `humidor`, `update`, `pin`, `errors`, `config`, ...), but `cli::run` is private, the crate has no feature flags, and a release `cig` is 13.0 MB (`cig-v1.20.0-linux-x86_64`). Linking the whole crate would put the installer past its own 10 MB budget (#20) before it does anything.
2. cig's tin is garbage collected by what run records reference (`tin::referenced`, `tin::gc`). Anything else stored there would be deleted by `cig tin gc`.
3. cig already dispatches pinned scripts through the humidor (`CIG_DISPATCHED=1`, `humidor::dispatch`), and honors `CIGSCRIPT_HOME` and `CIG_UPDATE_MANIFEST`, which makes tests and fixtures possible without touching the real home.
4. cigcompile's own comments say why the digests are compiled in: a checksum fetched from the same release as the asset proves nothing against a tampered release.

## Decision

### 1. One static binary, two helpers

`ciginstaller` (alias `cigi`, the same file under two names) does all the work. A second, tiny binary, `cigi-shim`, is what sits in `~/.cigscript/bin/` under the names `cig`, `cigo`, `cigcompile` and `cigar`. The shim uses only the standard library so it starts in well under 2 ms and never needs the installer's dependencies.

### 2. A Rust workspace of small crates, no link to `cigscript`

Crates: `cig-compat` (versions, manifest, marry-up checks; pure), `cigi-home` (paths, store, toolchains, journal, state, locks), `cigi-source` (release sources, downloads, trust), `cigi-ui` (the output grammar, palette, prompts, errors), `cigi-shim`, `ciginstaller`, and a dev-only `cigi-testkit` (fixture release source).

The installer does not depend on `cigscript`. It repeats the small, stable pieces it needs (the pin dialect, asset naming, the sidecar format, the humidor index, the registry schema) and a parity test, run off the main path, proves each still agrees with a pinned cig tag. Section "Alternatives" says why. The cost is real: some duplication. The shared crates that issue #1 imagines (`cig-journal`, `cig-tin`, `cig-errors`) become follow-up work in the CigScript repository, and the installer adopts them when they exist.

### 3. One home, and the installer's own store

Everything lives under `~/.cigscript/` (or `CIGSCRIPT_HOME`). New directories: `bin/`, `toolchains/`, `store/`, `installer/`. Existing ones (`runs`, `tin`, `humidor`, `config`) are left alone, apart from the humidor mirror below.

- **The store is `~/.cigscript/store/`, not the tin and not the humidor.** It is content addressed by SHA-256, one object per distinct file. A toolchain directory (`toolchains/<tool>/<version>/`) is built from hard links to store objects (a copy where links fail), made read-only. Two cigo versions share every file that did not change (docs, licenses, launchers) and cost one copy of each. Binaries that differ throughout do not dedup, and the design does not pretend they do; the #20 budget ("five cigo versions under 1.5x one") is about cigo, which is mostly text.
- **The humidor is mirrored, not replaced.** For each installed cig, the installer hard links the binary to `humidor/<ver>/cig` and adds an `index.json` entry (`source = "ciginstaller"`). cig's own per-script pin dispatch then finds every installed cig with no change to cig.

### 4. Cartons are the unit; the manifest is data

A carton is one version number that names one tested build of each product (#3). One TOML document, the manifest, lists the cartons, each product build's artifacts and digests, packs, requirements and yanks (#2). The installer reads it through `cig-compat`; no other code parses it.

### 5. Shims choose; the resolver explains

The shim's whole job: pick a carton, look up that carton's path for the tool in a small generated text file (`installer/shim-index`), and `exec` it. It picks the carton from `CIGI_CARTON`, then the nearest `.cig-carton` (or a `carton` key in `cig-tasks.json` / `cigar.toml`) above the working directory, then the machine's carton. If it finds an old per-tool pin file and no carton pin, it hands over to `cigi __resolve`, which does the full resolution (#6) and is allowed to take longer. The shim never parses TOML or semver.

### 6. Every change is a journaled transaction with one atomic flip

Installing, updating, switching, repairing and uninstalling run as a transaction in the installer's own journal (`installer/txn/<id>/`). Everything before the last step is additive (new store objects, new directories, new links) and invisible to the shims. The last step is a single `rename` of `shim-index`. A kill before the flip rolls back to the previous state; a kill after it rolls forward to the new one. There is no moment when a tool is half-updated.

The journal's vocabulary (`mkdir`, `write`, `link`, `move`, `delete`, with a "before" state for each) copies the names of cig's `Op` and `Before` so it can be moved onto cig's journal later. `cig unburn <update run>` (#7) is not in this build; it needs a `cig-journal` crate and a run record the installer can write. That is a follow-up.

### 7. Trust today is pinned digests plus two independent checksums; TUF plugs in behind one trait

We have no signing keys and no place to host TUF metadata, so today's guarantees are the ones cig and cigcompile already give, stated plainly: integrity, not authorship. Every artifact is admitted at the highest level it can reach:

1. **pinned**: its SHA-256 is compiled into this installer build (`pins.rs`, generated like cigcompile's `digests.rs`);
2. **attested**: `gh attestation verify` passes against the product's repository, and the digest matches;
3. **checksummed**: the digest in the manifest equals the digest in the product release's own `.sha256` sidecar, which are published from two different repositories, and equals the downloaded bytes.

Nothing below that installs (a local fixture directory may be marked `--allow-unverified` for development). The manifest carries `sequence` and `expires`; the installer keeps the highest `sequence` it has seen and refuses lower ones, which is a partial rollback defense that costs nothing.

TUF (#9) enters through one trait, `Trust`, whose implementations alone can produce an `Admitted<T>` (the only type the install and update code accepts). Today's implementation is `DigestTrust`; `TufTrust` (the `tough` crate, evaluated in #9) replaces it with the same signatures. Nothing else changes.

### 8. HTTP in process, GitHub release URLs, no API

The installer carries its own HTTPS client (`ureq` with rustls) instead of shelling out to `curl` or `gh`, because it is the program that has to work on a machine with nothing else installed, and because an in-process client can be tested against a local server. It downloads exact asset URLs named in the manifest, so it never calls the GitHub API and never hits its rate limits. `gh` is used only when present, for attestation checks, as cig does today.

### 9. No telemetry, as a testable property

Requests are plain `GET`s of public URLs with a fixed `User-Agent: ciginstaller` and no query strings, cookies or identifiers. A test records every request of a full install, update, and uninstall against the fixture server and fails on anything else (#19).

### 10. Errors, output and tests follow the house

Codes are `E1800`-`E1899`, in the family's registry format, with a registry file of our own (`errors/registry.toml`) that cig's registry will list under a new `installer` range. Output is the grammar of #32 (headline, detail, next step) with `--json`, `--plain`, and `NO_COLOR`. Tests run against a scratch `HOME` and a local fixture release source and never touch the network.

## Consequences

Good:

- Two projects can run two different cigo builds from one shim, chosen by the project's pin.
- An interrupted install or update cannot leave a half-updated tool.
- The installer works when cig is missing or broken, and stays under its size budget.
- Existing tools keep working untouched: cig's humidor dispatch and its `.cig-version` pins keep doing their job.
- The trust model is honest and upgradeable: one trait to swap.

Costs and risks:

- Duplicated logic (pin dialect, asset names, registry schema). Mitigated by the parity test and by moving to shared crates when cig extracts them.
- `cig unburn` of an update, the cigo/cigcompile trials, TUF, the window, and the reporting pipeline are not in this build (see `SPEC.md`, section 15).
- Shims add one exec hop. Measured budget: under 2 ms; a CI check enforces it.
- Hard links mean a tool that rewrites its own files in place (cigo's `update`, cig's `update`) would otherwise change a store object. Toolchain files are read-only and the shim exports `CIGI_MANAGED=1`; the follow-up change makes each updater defer to the installer when it sees that. Until then an in-place update fails loudly instead of corrupting the store, and `repair` restores the file.
- `ureq` and rustls add to binary size. The target is under 10 MB, checked by a spike before the first release (see `SPEC.md`, phase 0).

## Alternatives considered

**Link the `cigscript` crate and reuse `burn::journal`, `humidor`, `errors`.** This gives the real journal and `cig unburn` for free. Rejected for this build: the cig binary is 13 MB, the crate has no features, keyring and async-secret-service dependencies make static builds harder, the installer must run when cig is absent, and every cig release would force an installer rebuild to stay in step. Revisit once `cig-journal` is extracted; the `Journal` trait in `cigi-home` is the seam.

**Use the tin as the store.** It already dedups. Rejected: `cig tin gc` deletes unreferenced objects, and the tin chunks and seals files for a different purpose (undoing script runs). Mixing the two means one tool's cleanup breaks the other's installs.

**Symlinks instead of a shim.** Symlinks cannot choose by project. rustup and mise both use shims for this reason.

**Shim as a shell script.** Slower (a shell start is 1-5 ms), and wrong on Windows. The shim is a native binary.

**Shell out to `curl` / `gh` for downloads, like cig.** Smaller binary, but the installer would depend on tools it exists to bootstrap, and the HTTP path would be hard to test. Kept as a fallback only if the binary-size spike fails (`SPEC.md`, phase 0).

**One manifest per carton, or a database.** One document is simpler to sign, cache and read offline, and `cig-compat` answers every question from it. Size is no concern: a few KB per carton, and #26 keeps three live releases per product.

**Lockstep versions now (every product at the carton's number).** Left for the decision #3 asks to be recorded before carton 2.0.

**Trust the release `.sha256` alone.** That is what cig does today. We require it to agree with the manifest, which is published from a different repository, and prefer pinned digests and attestations.

## Open, and provisional until the operator decides

Each is built to its default and can change without touching the rest:

- **HTTP client.** In process (`ureq` with rustls). The size spike settled the budget question: a release binary with every heavy dependency (clap, ureq with rustls, toml, tar, flate2, serde_json) is 2.9 MB for x86_64 Linux (glibc), far under the 8 MB the plan allows. `curl` subprocesses behind the same `Source` trait stay the fallback.
- **Carton numbers.** `0.N.P` until carton 1.0.0 (#25).
- **Where the manifest is published.** A rolling release tagged `manifest` on this repository, assembled by hand with `cargo xtask manifest assemble` until a carton release job exists. A separate repository is the alternative. Trust today is integrity, not authorship (section 7).
- **The `cigscript` parity tests.** Behind a `parity` feature, run by hand and on tags, not on every push.
- **The first carton's contents.** cig, cigo and cigcompile; cigarscript joins when it has a release, and `tested = []` is written honestly until the integration suites exist.

## Measured before building (phase 0)

- **Size:** 2.9 MB for the dependency set above (release profile, x86_64 glibc). The static musl build needs a musl C compiler for `ring`; the release job builds it through `cross`.
- **Shim:** a median 0.24 ms from start to `exec` on the shim's own clock, over 1,000 runs, with a 2 KB index and a pin file found 40 directories up; 0.89 ms for the whole spawn of the shim running `/bin/true`, where a bare `/bin/true` spawn takes 0.25 ms. The shim binary is 344 KB. A test keeps asserting the budget.
- **Real releases:** every artifact of cig 1.20.0 (8 targets), cigcompile 0.2.0 (8 targets) and cigo 1.17.0 (one archive) matches its published `.sha256`; a manifest in this format was assembled from them. cigo's archive carries a `YANKED` list at its root, which the manifest assembly must read.
