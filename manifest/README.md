<!-- SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer) -->
<!-- SPDX-License-Identifier: CC-BY-4.0 -->
# The manifest's inputs

A manifest (SPEC section 6) is assembled, never written by hand.
- `tools.toml` says how each product is distributed.
- `products.toml` says what each product build needs.

Both are changed by pull request, like code. `cargo xtask manifest assemble` reads them, carries the previous manifest forward, and reads each new build's digest from its product's own release. It then writes the next manifest and its `.sha256` sidecar:

```sh
gh release download manifest -R otmof-ops/cig-installer -p manifest-stable.toml   # the current one, if any
cargo xtask manifest assemble --carton 0.1.0 --cig 1.20.0 --cigo 1.17.0 --cigcompile 0.2.0 \
  --pack dev=cig,cigo --pack builder=cig,cigo,cigcompile \
  --notes "first carton: cig, cigo, cigcompile" [--previous manifest-stable.toml]
```

It refuses in any of these cases:
- a sidecar is missing, or disagrees with GitHub's own digest of the asset;
- a version is yanked;
- a product's `requires` does not hold inside the carton;
- a published build would be fetched differently;
- the carton number is already taken.

Read its report before uploading. `cargo xtask pins <manifest>` then writes the compiled-in pins (`crates/cigi-source/src/pins_generated.rs`) from a manifest you have reviewed.

## Channels

Each channel has its own manifest, `manifest-<channel>.toml`, and its own sequence. Nothing is shared between them.
- `stable`: released cartons, numbered `X.Y.Z`. It takes no prerelease build and no candidate number.
- `beta`: the next carton's candidates, `X.Y.Z-beta.N`.
- `nightly`: a carton of every product's newest build, prereleases included, `X.Y.Z-nightly.N`.

For a candidate, `--candidate beta|nightly --base X.Y.Z` numbers the carton and sets the channel. N is one more than the highest candidate of the same base in that channel's previous manifest (1 for the first). Another base, another channel's candidates and released cartons do not count, and a number is never reused. A tool you do not name takes its newest published build under `--latest` (a nightly implies it). A tool with no eligible release is left out, and the report says so.

```sh
gh release download manifest -R otmof-ops/cig-installer -p manifest-nightly.toml
cargo xtask manifest assemble --candidate nightly --base 1.2.0 --previous manifest-nightly.toml
```

A semver range does not match a prerelease unless it names one, so a candidate holding a prerelease fails on any `requires` that does not: write the range in `products.toml` to include it.

`.github/workflows/nightly.yml` does the same by hand-started run and keeps the manifest as an artifact for review. It publishes nothing. Tick `first` for a channel that has no manifest yet. It has no schedule: adding one is the maintainer's decision.

`tested = []` for every candidate until the family's integration suites run against a carton's exact builds.

## Pruning old releases

`cargo xtask release prune` plans the cleanup of [#26](https://github.com/otmof-ops/cig-installer/issues/26). Each product keeps its three newest releases (`--keep`) and every version that is pinned, by a carton of `--manifest` or by `--pin <tool>=<version>:<who>`; the rest would move to the archive. It prints the plan with the reason for each version, and changes nothing:

```sh
cargo xtask release prune --manifest manifest-stable.toml --pin cig=1.6.2:cig-orchestrator
```

Archiving for real (`--yes`) is refused until the archive index can be written and signed (#9). The index is `archive/index.json` (format 1, read by `cig-compat`), and a manifest names it with `[archive] index = "https://..."`. The installer reads it only when a build's release is gone, and accepts the archived copy only at the digest the manifest pins.
