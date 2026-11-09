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
