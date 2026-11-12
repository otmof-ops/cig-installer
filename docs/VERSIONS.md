<!-- SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer) -->
<!-- SPDX-License-Identifier: CC-BY-4.0 -->

# Versions roadmap

The family ships as **cartons**: one numbered set of builds that were
checked to work together. Each product keeps its own version too; the
installer's [`manifest/products.toml`](../manifest/products.toml) is the
one place that says which product build belongs in which carton.

This page is the plan for the next tags, in both forms. A row is a
condition. Every condition is a tick. A tag is stamped when its column
is all ticked, and the tick is only written in the pull request that
stamps the tag or the one that finishes the condition. The evidence a
row names (a run, a pull request, an issue closed) is the only reason a
box is ticked.

## The family's two rules

1. **A carton stays `0.N.P` until every product in it is at least 1.0.**
   Once every product has reached 1.0, the next carton is 1.0.0.
2. **Only versions in the installer ecosystem keep version tags on
   GitHub,** meaning the versions listed in
   [`manifest/products.toml`](../manifest/products.toml). Every other
   version tag is deleted with its release once carton 0.1.0 is out. A
   version leaves GitHub when it leaves the ecosystem. The
   `apache-2.0-last` tag always stays on every repository: it is the
   license boundary, not a version.

## Where things stand

Today, 2026-11-12. The carton is **carton 0.1.0**: cig 1.21.0, cigo
1.19.0, CigCompile 0.4.0, CigarScript 0.1.0, ciginstaller 0.1.0.

| product | current | kept on GitHub (from `products.toml`) | next milestone | what the number covers |
|---|---|---|---|---|
| **cig** (`cig-script`) | 1.21.0 | 1.20.0, 1.21.0 | 1.22.0 (the next minor) | the language surface ([`docs/LANGUAGE.md`](https://github.com/otmof-ops/cig-script/blob/main/docs/LANGUAGE.md)), the effect rules ([`docs/BURN.md`](https://github.com/otmof-ops/cig-script/blob/main/docs/BURN.md)), the stable exit codes and the diagnostic schema. Installing and updating belong to the installer and the carton, not to this number. |
| **cigo** (`cig-orchestrator`) | 1.19.0 | 1.17.0, 1.18.0, 1.19.0 | 1.20.0 (the next minor) | the guard model and the preset defaults ([`DECISIONS`](https://github.com/otmof-ops/cig-orchestrator/blob/main/orchestrate.cig) 16, 18), the `cig-tasks.json` schema, the agent hook contract, the detection rules and `cigo __doctor` findings. |
| **CigCompile** (`cig-compile`) | 0.4.0 | 0.2.0, 0.3.0, 0.4.0 | 0.5.0 (stage B lands) | the program's trailer and file layout ([`how-it-works.md`](https://github.com/otmof-ops/cig-compile/blob/main/docs/how-it-works.md)), the differential-suite contract (a program runs byte for byte like `cig run`), the cig runtime it embeds, and the SBOM, signing and audit outputs. |
| **CigarScript** (`cigar-script`) | 0.1.0 | 0.1.0 | 0.2.0 (the first set of fixes on 0.1.0) | the three strengths and the superset guarantee ([`roadmap.md`](https://github.com/otmof-ops/cigar-script/blob/main/docs/roadmap.md)): nothing touches the world outside a burn at every strength; a `.cig` file stays valid; turning a strength up is additive; `E1102` marks what is not built. |
| **ciginstaller** (`cig-installer`) | 0.1.0 | 0.1.0 | 0.2.0 (the next minor) | the install, pin, update and rollback contract, the shim's budget, the carton manifest, the trust levels, the error range `E1800`-`E1999`, the `--json` shapes ([`docs/JSON.md`](JSON.md)) and the journal ([`docs/JOURNAL.md`](JOURNAL.md)). |

Everything else that is in any repository's tag history is outside the
ecosystem and comes off GitHub once carton 0.1.0 is out, by the second
rule.

## The gate every tag passes

Every release of every product passes the same gate before it is tagged.
Each row is ticked when the evidence is in place today.

| condition | cig | cigo | CigCompile | CigarScript | ciginstaller | evidence |
|---|:---:|:---:|:---:|:---:|:---:|---|
| CI green on every platform the product supports, no rerun of a known flake | [ ] | [ ] | [ ] | [ ] | [ ] | the release's workflow run; the three known flakes below |
| the license gate passes | [x] | [x] | [x] | [x] | [x] | FSL-1.1-ALv2 for every release in `products.toml`; the earlier Apache-2.0 period is frozen at `apache-2.0-last` |
| the CHANGELOG has a section for this release | [x] | [x] | [x] | [x] | [x] | each repository's `CHANGELOG.md` |
| the docs match (README, `docs/`, `LANGUAGE.md`, `BURN.md`, `BENCH.md` as applicable) | [x] | [x] | [x] | [x] | [x] | the release's own PR |
| a `products.toml` entry with `requires`, `license` and (where it applies) `embeds` | [x] | [x] | [x] | [x] | n/a | [`manifest/products.toml`](../manifest/products.toml) |
| sha256 and attestation on every asset the release carries | [x] | [x] | [x] | [x] | [x] | each release's artifacts and `gh attestation` |
| the release notes are the CHANGELOG section (no new text) | [x] | [x] | [x] | [x] | [x] | the release's body |
| the bench gate holds, or an accepted exception is in the release notes | [ ] | [ ] | [ ] | [ ] | [ ] | the bench workflow's run; thresholds in each repo's `docs/BENCH.md` or `docs/BENCHMARKS.md` |

The "n/a" is the installer itself: `products.toml` names the builds the
installer manages, not the installer.

### Bench thresholds, per repo

| repo | gate file | fail on |
|---|---|---|
| cig | [`bench/bench.toml`](https://github.com/otmof-ops/cig-script/blob/main/bench/bench.toml), [`docs/BENCH.md`](https://github.com/otmof-ops/cig-script/blob/main/docs/BENCH.md) | `correctness`; `disk` (bytes per byte burned more than 10% over the case's `reference`). `start-up` and `regression` are reported, not gated, on shared runners (#153) |
| cigo | [`bench/bench.toml`](https://github.com/otmof-ops/cig-orchestrator/blob/main/bench/bench.toml), [`docs/benchmarks.md`](https://github.com/otmof-ops/cig-orchestrator/blob/main/docs/benchmarks.md) | `correctness`; wall times are reported, not gated, on shared runners (#153). The detection and latency gates are #88 and #90, the hook's cache is #117 |
| CigCompile | [`docs/BENCHMARKS.md`](https://github.com/otmof-ops/cig-compile/blob/main/docs/BENCHMARKS.md) ([#30](https://github.com/otmof-ops/cig-compile/issues/30)) | `correctness`; `regression` (5% median or 2% geomean, non-noisy); the per-stage budgets of [#14](https://github.com/otmof-ops/cig-compile/issues/14); memory 10% over `main` |
| CigarScript | [`bench/bench.toml`](https://github.com/otmof-ops/cigar-script/blob/main/bench/bench.toml) ([#17](https://github.com/otmof-ops/cigar-script/issues/17)) | `correctness`; the superset no slower than CigScript; strength-check cost (the other groups land with the features they measure) |
| ciginstaller | [`bench/budgets.toml`](../bench/budgets.toml), [`docs/BENCH.md`](BENCH.md) | the 11 budgets of [#20](https://github.com/otmof-ops/cig-installer/issues/20) and [#32](https://github.com/otmof-ops/cig-installer/issues/32), on Linux and macOS; Windows measures the four that do not need a shell-script product |

## Carton 0.2.0

The next carton. Each row is a condition on the carton. A row is ticked
when its evidence is in place today.

| # | condition | evidence | state |
|---|---|---|:---:|
| 1 | cig's `run_start_is_constant_in_history` passes on Windows | `cig-script` `src/burn/runs.rs:716`; the test and the run that proves it on `windows-latest` and `windows-11-arm` | [ ] |
| 2 | the installer's `cigi-ui::terminal::a_silent_terminal_gets_the_neutral_theme_within_the_budget` passes within its Linux budget | `crates/cigi-ui/tests/terminal.rs:282`; the test holds 150 ms on Linux and 500 ms on macOS | [ ] |
| 3 | the installer's `cigi-ui::detect::tests::a_deadline_already_past_reads_nothing` passes on macOS | `crates/cigi-ui/src/detect.rs:567` | [ ] |
| 4 | cigo's `rehearse --jobs` race (E759) fixed | the cigo PR that lands the fix. Seen in CI on cig-orchestrator#132 (run 43151999783, both Linux jobs): the test `rehearse --jobs: packages are rehearsed side by side, each in its own box, and print in the order asked (#56)` failed with E759 because a run started earlier in the suite still held the repository's pack; the rerun passed | [ ] |
| 5 | path filters for docs-only changes on every repo's `ci.yml` | each workflow file's `paths-ignore` for `docs/**` and `*.md` where the job does not build or test docs | [ ] |
| 6 | `concurrency: cancel-in-progress: true` for pull-request runs on every repo's `ci.yml` | each workflow file's `concurrency` block | [ ] |
| 7 | release jobs reuse the commit's green CI checks | each `release.yml` reads the pushed tag's SHA's own run and does not rebuild and retest | [ ] |
| 8 | a pinned toolchain (`rust-toolchain.toml` or a pinned minor in the workflow) in every repo | today only `cig-installer/rust-toolchain.toml` exists (`channel = "stable"`, not pinned); the four others have no file | [ ] |
| 9 | CigCompile 0.4.1: compiled start-up back to the 0.3.0 level | `cigcompile bench --against main`: median `startup-0` compiled about 3.5 ms (today 4.7 ms, +40.6%); cig-compile run 43167882002 | [ ] |
| 10 | CigCompile 0.4.1: the bench gate passes against `main` with no accepted exception | the bench workflow's run; today peak memory is +10% on every program (mimalloc on musl, cig 1.21.0 [#146](https://github.com/otmof-ops/cig-script/issues/146)) and is an accepted exception on 0.4.0 | [ ] |
| 11 | a new CigarScript release pins cig 1.21.0 | `cigar-script` `Cargo.toml` pins `cigscript` at tag `v1.21.0`. Today CigarScript 0.1.0's `Cargo.toml` pins `v1.20.0` ([`products.toml`](../manifest/products.toml) `[product.cigar."0.1.0"]`), and `main` pins the same; moving it to `v1.21.0` takes the `AssignTarget::Var` change at about eight call sites | [ ] |
| 12 | cig 1.20.0 leaves the ecosystem (rule 2) | once no version in `products.toml` requires it: today CigCompile 0.3.0 requires `cig = "=1.20.0"`, so cig 1.20.0 drops only when CigCompile 0.3.0 drops too. When both go, the tag and release are deleted; `apache-2.0-last` stays | [ ] |
| 13 | the gate above passes for every product in the carton | the gate rows ticked for cig, cigo, CigCompile, CigarScript and ciginstaller | [ ] |
| 14 | the install matrix ([#21](https://github.com/otmof-ops/cig-installer/issues/21)) green against carton 0.2.0 on debian, ubuntu, parrot, fedora, arch and alpine | the `fresh-install` job's run; the upgrade-from-0.1.0 job once it lands | [ ] |

## ciginstaller 1.0

| # | condition | evidence | state |
|---|---|---|:---:|
| 1 | the signed update channel is on by default ([#9](https://github.com/otmof-ops/cig-installer/issues/9)) | a non-placeholder TUF root compiled in; the `tuf` workflow signs; `trust = signed` is the floor | [ ] |
| 2 | macOS builds ([#5](https://github.com/otmof-ops/cig-installer/issues/5)) | the macOS release target is attached to every release, with sha256 and attestation | [ ] |
| 3 | Windows builds ([#5](https://github.com/otmof-ops/cig-installer/issues/5)) | the Windows release target is attached; `install.ps1` no longer stops; the Windows runtime ([#8](https://github.com/otmof-ops/cig-installer/issues/8)) is in | [ ] |
| 4 | platform packages ([#15](https://github.com/otmof-ops/cig-installer/issues/15)) | apt, dnf, AUR, Homebrew, winget and Scoop manifests, each built in CI from the same signed release | [ ] |
| 5 | the install matrix green across Linux, macOS and Windows ([#21](https://github.com/otmof-ops/cig-installer/issues/21)) | fresh installs, upgrades from every previous carton, mismatch warnings, and the attack suite of [#9](https://github.com/otmof-ops/cig-installer/issues/9) | [ ] |
| 6 | the fresh-install workflow proves install, status, a shim, doctor and the single-source refusal on every distro | `.github/workflows/ci.yml` `fresh-install` job; present and green today for a fixture mirror, to extend to cartons from a mirror and from a bundle | [ ] |
| 7 | the window ([#18](https://github.com/otmof-ops/cig-installer/issues/18)) is built, keyboard-first, accessible | the window's own tests on Linux, macOS and Windows | [ ] |
| 8 | the courier ([#24](https://github.com/otmof-ops/cig-installer/issues/24)) is built | reports file with no `gh` and no `CIG_GITHUB_TOKEN` through the courier, with a scoped token, and no identifier leaves the machine | [ ] |
| 9 | the family's bug flow finished ([#29](https://github.com/otmof-ops/cig-installer/issues/29)) | similarity search, "add mine", `bug status` and `bug withdraw`, evidence for cigo, the guard, CigCompile and updates | [ ] |
| 10 | the signed compatibility index ([#30](https://github.com/otmof-ops/cig-installer/issues/30)) is wired and `which --why` reads it | a signed index served; `ciginstaller programs` reads it; the fixed-bug announcement runs across the family | [ ] |
| 11 | CIG.OS Secure Boot ([#23](https://github.com/otmof-ops/cig-installer/issues/23)) decided and documented | the decision record; the choice honored | [ ] |
| 12 | `ciginstaller repair` and `uninstall --all` leave nothing behind, on every platform ([#14](https://github.com/otmof-ops/cig-installer/issues/14)) | the suite that proves it | [ ] |
| 13 | every bench budget of [`bench/budgets.toml`](../bench/budgets.toml) passes on every platform the row names | the `bench` workflow's run on Linux, macOS and Windows; Windows no longer prints "NOT MEASURED" for a row it must measure | [ ] |
| 14 | the release-install budget passes from a real mirror, not only a local fixture | `bench --from` against a mirror served by the release workflow | [ ] |
| 15 | the standards sweep ([#34](https://github.com/otmof-ops/cig-installer/issues/34)) runs in this repo's CI | `ciginstaller standards check` passes; the check is called from `ci.yml` | [ ] |

## CigarScript 1.0

| # | condition | evidence | state |
|---|---|---|:---:|
| 1 | design question 5 (calling Rust and C) decided ([cigar-script#5](https://github.com/otmof-ops/cigar-script/issues/5)) | the decision record; the grammar is no longer "provisional" where this touches it | [ ] |
| 2 | design questions 1-4 decided | [cigar-script#1](https://github.com/otmof-ops/cigar-script/issues/1), [#2](https://github.com/otmof-ops/cigar-script/issues/2), [#3](https://github.com/otmof-ops/cigar-script/issues/3), [#4](https://github.com/otmof-ops/cigar-script/issues/4) are closed and the decisions are reflected in the language reference | [x] |
| 3 | `E1102` gaps closed: `foreign`, `unchecked` blocks, every `within` context, custom journaling adapters, cross-script reads | [cigar-script#5](https://github.com/otmof-ops/cigar-script/issues/5), [#12](https://github.com/otmof-ops/cigar-script/issues/12), [#13](https://github.com/otmof-ops/cigar-script/issues/13), [#14](https://github.com/otmof-ops/cigar-script/issues/14); the strength docs no longer list anything as "not built"; `cigar check` no longer emits `E1102` on a conforming file | [ ] |
| 4 | held to Rust's standard ([cigar-script#22](https://github.com/otmof-ops/cigar-script/issues/22)) | the 8 soundness holes closed; the 9 strictness gaps closed; the 3 design calls made | [ ] |
| 5 | the rest of the final language ([cigar-script#38](https://github.com/otmof-ops/cigar-script/issues/38)) | constraints and generic impls; standard interfaces with operators, iteration and derive; full patterns and destructuring; tuples, newtypes, constants; test blocks and doc comments; what it deliberately won't have | [ ] |
| 6 | `carton` packages ([cigar-script#15](https://github.com/otmof-ops/cigar-script/issues/15)) | installable packages, no install scripts | [ ] |
| 7 | `cigar-lsp` and editor support ([cigar-script#16](https://github.com/otmof-ops/cigar-script/issues/16)) | the language server, the formatter, the editor extensions | [ ] |
| 8 | benchmarks from the first commit ([cigar-script#17](https://github.com/otmof-ops/cigar-script/issues/17)) | `bench/bench.toml` groups for every feature; the superset is no slower than CigScript; types for speed measured | [ ] |
| 9 | the language reference stops carrying "provisional" | `docs/LANGUAGE.md` and `docs/roadmap.md` at the tag | [ ] |

## CigCompile 1.0

| # | condition | evidence | state |
|---|---|---|:---:|
| 1 | stage B lands: a bytecode VM at least 3× the tree-walker, CPython parity at the set's median | [cig-compile#31](https://github.com/otmof-ops/cig-compile/issues/31), [cig-compile#14](https://github.com/otmof-ops/cig-compile/issues/14); differential zero differences | [ ] |
| 2 | stage C lands: native code through Cranelift, 15× the tree-walker and 3× CPython, within 2× Node on `nbody`, `mandel`, `spectral`, `loop` | [cig-compile#32](https://github.com/otmof-ops/cig-compile/issues/32), [#40](https://github.com/otmof-ops/cig-compile/issues/40)'s mitigations | [ ] |
| 3 | stage D lands: typed lowering, within 3× Rust on `loop`, `mandel`, `nbody`, `sieve`, `fib` | [cig-compile#34](https://github.com/otmof-ops/cig-compile/issues/34); CigarScript's declared types read | [ ] |
| 4 | start-up and size: a compiled hello at under 2 MB and 2 ms | [cig-compile#36](https://github.com/otmof-ops/cig-compile/issues/36), [#14](https://github.com/otmof-ops/cig-compile/issues/14) | [ ] |
| 5 | compiled start-up on 0.4.0's runtime is no worse than 0.3.0's | the bench workflow's run; today `startup-0` 3.4 → 4.7 ms (+40.6%), `startup-1` 3.5 → 5.4 (+54.2%), `startup-10` 3.7 → 5.6 (+51.5%), `hello` 3.2 → 4.1 (+25.4%) on cig 1.21.0 (run 43167882002), carried into 0.4.1 by carton 0.2.0 row 9 | [ ] |
| 6 | compiled peak memory has an explicit budget | the `memory` gate of [cig-compile#30](https://github.com/otmof-ops/cig-compile/issues/30); today +~3.5 MB on every program (24.8 → 28.4 MB), data-heavy worse (`json` 52.3 → 73.5 MB, `lists` 24.8 → 35.8 MB) under mimalloc on musl (cig 1.21.0 [#146](https://github.com/otmof-ops/cig-script/issues/146)) | [ ] |
| 7 | hardened programs ([cig-compile#40](https://github.com/otmof-ops/cig-compile/issues/40)) | CFI, RELRO, no RWX, reproducible `verify`, an SBOM in every program, `cigcompile audit` reads them back on every target | [ ] |
| 8 | reproducible builds held by CI | the differential suite is reproducible on every target; the "same inputs, same bytes" test | [ ] |
| 9 | more targets and outputs ([cig-compile#41](https://github.com/otmof-ops/cig-compile/issues/41)) | WASI, RISC-V, Android, a library with a C header, OCI images and installer packages | [ ] |
| 10 | debugging and profiling ([cig-compile#38](https://github.com/otmof-ops/cig-compile/issues/38)) | backtraces in pull names, gdb/lldb/WinDbg, a built-in profiler, `--emit` for every stage | [ ] |
| 11 | build speed ([cig-compile#37](https://github.com/otmof-ops/cig-compile/issues/37)) | per-function caching, parallel codegen, incremental builds, watch mode and `--timings` at the budgets of [#14](https://github.com/otmof-ops/cig-compile/issues/14) | [ ] |
| 12 | the stable program format is documented and carries the checked tree ([cig-compile#71](https://github.com/otmof-ops/cig-compile/issues/71)) | the trailer and the top-level snapshot are a documented contract; compiled `cigo --version` under 5 ms | [ ] |
| 13 | self-hosting ([cig-compile#17](https://github.com/otmof-ops/cig-compile/issues/17)) | CigCompile in CigarScript compiles itself to identical bytes | [ ] |
| 14 | correctness at scale ([cig-compile#39](https://github.com/otmof-ops/cig-compile/issues/39)) | `cig-smith` random programs; differential fuzzing across stages and levels; reduction and translation validation | [ ] |
| 15 | CigCompile runs on `cig bench` with the family harness ([cig-compile#68](https://github.com/otmof-ops/cig-compile/issues/68)) | `bench/bench.toml` in the shape of cig-script's; the build, size and start-up cases shared with cig | [ ] |

## Carton 1.0.0

Every product in the carton is at 1.0 or above, and the family's own
integration suites run against exactly this carton's builds.

| # | condition | evidence | state |
|---|---|---|:---:|
| 1 | cig at 1.x, open on `security` labels closed | [cig-script#106](https://github.com/otmof-ops/cig-script/issues/106) (the box), [#115](https://github.com/otmof-ops/cig-script/issues/115) (`cig net`), [#146](https://github.com/otmof-ops/cig-script/issues/146), [#147](https://github.com/otmof-ops/cig-script/issues/147) (burn throughput), [#188](https://github.com/otmof-ops/cig-script/issues/188) (wait times) landed as needed; [#150](https://github.com/otmof-ops/cig-script/issues/150)'s table is measured | [ ] |
| 2 | cigo at 1.x, the guard suites green | [cig-orchestrator#55](https://github.com/otmof-ops/cig-orchestrator/issues/55)-[#66](https://github.com/otmof-ops/cig-orchestrator/issues/66) landed as the preset demands (guard, firmware, spyware, blacklist, scope, prompt tiers, offsite backup, spare); [#88](https://github.com/otmof-ops/cig-orchestrator/issues/88)'s detection corpus with precision and recall gates green; [#90](https://github.com/otmof-ops/cig-orchestrator/issues/90)'s latency targets held; [#117](https://github.com/otmof-ops/cig-orchestrator/issues/117)'s verdict cache in; [#119](https://github.com/otmof-ops/cig-orchestrator/issues/119)'s macOS and Windows slowdown fixed | [ ] |
| 3 | CigCompile at 1.0 (above) | the matrix above all ticked | [ ] |
| 4 | CigarScript at 1.0 (above) | the matrix above all ticked | [ ] |
| 5 | ciginstaller at 1.0 (above) | the matrix above all ticked | [ ] |
| 6 | the family's integration suite runs against exactly carton 1.0.0's builds, on every platform in [#21](https://github.com/otmof-ops/cig-installer/issues/21) | each manifest's `tested = []` becomes `tested = [<run id>]`; today the nightly workflow (`.github/workflows/nightly.yml`) leaves it empty on every candidate carton | [ ] |
| 7 | old pins still work ([cig-installer#6](https://github.com/otmof-ops/cig-installer/issues/6)) | a script with `# cig >=1.6.2` or `.cig-version`, a cigo version floor, a `cigar.toml` `cigar` key, each resolves to a carton 1.0.0 build or says which carton it needs | [ ] |
| 8 | release cleanup ([cig-installer#26](https://github.com/otmof-ops/cig-installer/issues/26)) | three versions per product on GitHub (plus any version a supported carton pins); the rest archived; the signed `archive/index.json` published and reachable; the installer fetches from it on a gone release (E1825 on a mismatch) | [ ] |
| 9 | the licensing review ([cig-installer#27](https://github.com/otmof-ops/cig-installer/issues/27)) decides the terms carton 1.0.0 ships under | the decision record; every release from then on under those terms | [ ] |
| 10 | each product's README names ciginstaller as the way to install | each repo's README at its carton 1.0.0 build | [ ] |
| 11 | one doctor ([cig-installer#13](https://github.com/otmof-ops/cig-installer/issues/13)) and one ashtray with the courier ([cig-installer#10](https://github.com/otmof-ops/cig-installer/issues/10), [#24](https://github.com/otmof-ops/cig-installer/issues/24)) carry reports for the whole family | the family suite that proves it | [ ] |
| 12 | fresh install, upgrade from every carton in the stable channel, and uninstall that leaves nothing behind | the `fresh-install` and upgrade jobs on every platform | [ ] |
| 13 | the signed update channel ([#9](https://github.com/otmof-ops/cig-installer/issues/9)) has its key ceremony done; the compiled-in root is not a placeholder | the TUF root in the binary; the `tuf` workflow signed releases | [ ] |

## Triggers for the next major in each product

A major version is a trigger, not a target. A script or a project that
worked stops working; that forces the next major, and nothing else does.

### cig: if forced, **3.0.0**, not 2.0.0

A private archive already holds cig 2.0.0-2.1.0, so when the next major
on GitHub comes, it skips to 3.0.0.

A change that forces it:

- a script that was valid stops being valid (the language surface in
  [`docs/LANGUAGE.md`](https://github.com/otmof-ops/cig-script/blob/main/docs/LANGUAGE.md)
  changes in a way a pin cannot cover);
- the effect rules in
  [`docs/BURN.md`](https://github.com/otmof-ops/cig-script/blob/main/docs/BURN.md)
  change in a way a plan or a journal from the old cig cannot be read
  by the new cig;
- a stable exit code or the diagnostic schema changes in a way a tool
  reading them breaks.

Installing and updating moving to the installer is **not** a trigger:
cig 1.21.0 moved it and is a minor.

### cigo: if forced, **2.0.0**

A change that forces it:

- the guard model or the preset defaults
  ([`DECISIONS`](https://github.com/otmof-ops/cig-orchestrator/blob/main/orchestrate.cig)
  16, 18) change in a way an existing `cigo config` cannot carry over;
- the `cig-tasks.json` schema changes in a way a project's own
  `cig-tasks.json` stops being read;
- the agent hook's protocol changes in a way an agent integration
  built against 1.x stops working.

## How to use this doc

- A box is ticked `[x]` only with the evidence linked in the same pull
  request. Prose without a run, a pull request number or an issue
  closed is not evidence.
- A tag is stamped when its column is all ticked. A row that cannot be
  ticked and that the tag cannot wait on is written as an **accepted
  exception** in that tag's release notes, naming the row and why,
  and stays ticked `[ ]` here.
- The same pull request that finishes a condition ticks its row.
- When a product's number moves, the "where things stand" table and
  the row counts on the matrices are updated in that pull request too.
- A row is never deleted. If a condition no longer applies, it is
  struck through with an explanation on the line that follows.
