<!-- SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer) -->
<!-- SPDX-License-Identifier: CC-BY-4.0 -->
# The carton gate: a local carton, proven end to end

A carton is proven before anything of it is published. Two pieces do it:

- `cargo xtask carton-local` builds every product from a local source tree for one target and writes a directory that `ciginstaller install carton <v> --from <dir>` reads.
- `ci/carton-gate.sh <carton dir> [<cig-orchestrator tree>]` installs that carton into a scratch home with the installer the carton carries, then proves each product and the products together.

`ci/carton-e2e.sh` runs both, one after the other. The gate runs cig's burns and cigo's boxed tasks, so it runs on a test machine, never on a development machine.

## The carton directory

`cargo xtask carton-local --carton <v> --out <dir> --cig <tree> --cigo <tree> --cigcompile <tree> --cigar <tree> --installer <tree> [--work <dir>] [--target <rust-target>]`

- Each product is built the way its release workflow builds it, with the same file name, the same archive layout and a `.sha256` sidecar in `sha256sum`'s format. The asset names come from `manifest/tools.toml`.
- The target defaults to the release workflows' target for this machine; on Linux that is the static musl build (`x86_64-unknown-linux-musl` needs `musl-gcc`).
- The products are built against each other's trees, not the pinned releases. cigar is built against the local cig-script, and cigcompile against the local cig-script and cigar-script. Both use cargo patch overrides on the command line (`--config patch."https://github.com/otmof-ops/cig-script".cigscript.path=...`). A patched build rewrites its lock file, so it builds from a copy of its tree under `<work>/<tool>/src`; the trees themselves are never written.
- The manifest is written by `manifest assemble`'s own code. That code reads the directory as the release host: an asset's digest is the sha256 of the file, and each sidecar must agree with it.
- `<work>` (default `target/carton-local`) holds one cargo target directory per product, so a second build is warm.
- The directory also holds `carton-local.env`, `KEY=value` lines naming the carton, the target and each build. The gate reads it as data.
- The archives are made with GNU `tar` and `gzip`, as the release workflows make them, so the command runs on Linux.
- A local carton carries the version numbers in the trees. When those are numbers already published, its builds are other bytes under the same names. Keep its directory apart from the published manifest and never upload it.

### Trust

An install from the directory reaches the `checksummed` level and no higher:

- The directory is one source, which the installer refuses unless `allow_single_source` is set. The gate sets it the way a user allows a private mirror.
- No compiled-in pin names the local bytes, so `pinned` is not reached.
- No local build has an attestation, so `attested` is not reached. Where `gh` is present, each build's install notes that its attestation check failed.
- The manifest is not signed.

No check is weakened to get there.

## The gate's steps

Each step prints `ok <step>` or `FAIL <step>: <why>`. The run exits 1 when any step failed, and writes `gate.json` (the carton, its builds, the manifest's sha256 and each step's result) to `GATE_OUT`.

| step | what it proves |
|---|---|
| `installer-from-carton` | the carton's installer tarball matches its sidecar and runs |
| `install` | `install carton <v> --yes --from <dir>` in a fresh scratch home |
| `installed-is-the-manifest` | each binary is the asset byte for byte, and each archive's toolchain is the archive unpacked file for file; the manifest and the receipt name the same sha256 |
| `every-shim-answers` | `cig`, `cigo`, `cigcompile`, `cigar` and `cigar-lsp` on PATH are the carton's shims and answer with its versions; cigar-lsp has no `--version`, so its version is read from `initialize` |
| `doctor`, `check` | `ciginstaller doctor` and `ciginstaller check` find nothing wrong. A fresh home has no cigo guard, and cigo's doctor reports that as two actions (`guard-shims`, `guard-path`). Those two are named and allowed; any other finding fails the step |
| `cig-failed-burn-rolls-back` | a burn that writes two files and then fails is rolled back, and `cig runs` says `rolled_back` |
| `cig-runs-and-unburn` | a good run is listed `ok` by `cig runs`, and `cig unburn <id>` takes it back |
| `cigo-run-ci` | a project's `cigo run ci` passes |
| `cigo-failed-task-rolls-back` | a task that writes a file and fails leaves nothing |
| `cigo-task-sees-the-family-home` | a cigo task sees the same `HOME`, `XDG_CONFIG_HOME` and `CIGI_MIRROR`. Its `CIGSCRIPT_HOME` is the run's inbox, which is cig's state root there. `ciginstaller` run inside the task answers for the family home: its settings and its toolchains ([decision 0005](decisions/0005-the-home-a-tool-lives-in.md)) |
| `shims-answer-inside-a-task` | `cig`, `cigo`, `cigcompile`, `cigar` and `ciginstaller` run inside a boxed cigo task each answer with the carton's version |
| `install-inside-a-task-is-refused` | `ciginstaller install carton` inside a task stops with `E1905`, and the home's `bin/`, `toolchains/`, `installer/` and `humidor/` are byte for byte as before |
| `cigcompile-build-run-verify` | a hello and a burn are built with `--runtime` set to the carton's cig, run, and pass `cigcompile verify` |
| `cigcompile-finds-the-cartons-cig` | with no `--runtime` and no network, cigcompile finds the carton's cig by itself |
| `cigar-check-three-strengths` | `cigar check` on a mild, a medium and a full file |
| `cigar-run` | `.cigar` programs run and print what they should |
| `cigar-lsp-initialize` | the language server answers `initialize` and exits 0 after `shutdown` and `exit` |
| `products-together-polyglot` | cigo's polyglot example runs `ci` and `release` with the carton's cigcompile, cigar and ciginstaller on PATH |
| `lifecycle-install` | a second fresh home, for the installer's own lifecycle |
| `pin-is-honored` | a project pinned to the carton's cig runs it; a project pinned to a cig the carton lacks does not run the carton's |
| `repair-shim-and-toolchain` | `repair` puts back a deleted shim and a damaged toolchain file |
| `uninstall-all-leaves-nothing` | `uninstall --all --yes-irreversible` removes everything the installer added. It keeps the user's data and names what stays: cig's `runs/`, `locks/` and `runs-index.*`, and cig's key folder beside the home (`.cigscript-keys/`, the keys to the user's sealed data). The installer's own `gh` calls leave no state behind. Anything else left in the scratch home fails the step, and the log lists every path left |

Every installer call reads the carton's directory (`CIGI_MIRROR=file://<dir>`), including the ones cigo makes itself. The products run with their own test settings: `CIG_SEAL_KEY=file`, `CIG_DOCTOR=0`, `CIG_REPORTS=never`, `CIGCOMPILE_OFFLINE=1`, and cigo's `CIGO_FLOOR_CACHE`, `CIGO_OSV`, `CIGO_REGISTRY_META` and `CIGO_REHEARSE` turned off.

## Where to run it

On a machine that may make boxes: the gate runs cigo's runs and cig's burns, so run it on a test machine or in a throwaway VM, never on a development machine. It needs the five source trees, this repository, a Rust toolchain and the tools the products build with. It needs no credentials: it builds from the trees and fetches crates over https.

```sh
sh ci/carton-e2e.sh --work <work dir> --carton 0.2.0 \
  --cig <cig-script tree> --cigo <cig-orchestrator tree> --cigcompile <cig-compile tree> \
  --cigar <cigar-script tree> --installer <cig-installer tree>
```

- The script prints the gate's `ok` and `FAIL` lines and exits with the gate's code.
- The gate's whole log is `<work dir>/gate/gate.log`, and its summary is `<work dir>/gate/gate.json`.
- The carton directory is `<work dir>/carton`.
- Build output stays in `<work dir>/build/<tool>/target`, so the next run is warm.
- `--build-only` stops after the carton is built.
