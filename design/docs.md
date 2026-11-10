<!-- SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer) -->
<!-- SPDX-License-Identifier: CC-BY-4.0 -->

# House style for docs

Standard version 1.0.0. The style the family's READMEs already share, written down so the sweep (#34) can check it.

## README order

1. The logo, centred.
2. A one-line bold promise and an italic second line.
3. Badges: grey labels for facts (`#6b7280`), `ember` for identity (`#f97316`), green for the cheeky stat (`#2ea44f`: "nicotine 0 mg", "glue scripts: 0"). The colours are in [generated/badges.json](generated/badges.json).
4. A short hook in the voice.
5. A "thirty seconds" or "sixty-second tour" with commands.
6. What you get.
7. Install.
8. Commands.
9. Limits, honestly.
10. Part of the family.
11. Licensing.

## Every doc page

- An SPDX header (CC-BY-4.0 for docs, as `REUSE.toml` maps).
- One H1.
- Sentence-case headings.
- Tables for comparisons.
- Code blocks with the language.
- Each command shown with what it prints.
- No screenshots of text: render terminals as SVG, as `terminal.svg` does.
- American English, kebab-case file names.

## Diagrams

- 820 px wide.
- Rounded boxes with grey outlines, `ember` arrows and highlights, mono labels, as `burn-model.svg`, `chain.svg`, `stages.svg` and `glue-vs-chain.svg` are.
- Styled from [generated/svg-styles.css](generated/svg-styles.css), generated from the tokens.
- Generated from source where possible.

## Wordmarks

Mono, the product name in `ash`, the second word in `ember`: "Cig**Script**", "cig-**orchestrator**", "cig-**installer**".

## Files every repository has

README, CHANGELOG (Keep a Changelog), CONTRIBUTING (with DCO), SECURITY, SUPPORT, TRADEMARKS, LICENSE and LICENSES/, NOTICE, THIRD-PARTY-NOTICES, REUSE.toml, YANKED (where there are releases), and `docs/assets/logo.svg` plus a social preview at 1280 by 640.

## Errors

Documented from the registry (`errors/registry.toml` to `docs/ERRORS.md`), never by hand.

## Product names in prose

CigScript, CigCompile, CigarScript, cigo, cig-orchestrator, ciginstaller. In running text and in output the products are CigCompile and CigarScript; in commands, paths, flags and JSON the ids stay lowercase (`cigcompile`, `cigar`). The terminal UI does this itself (`product_names`).

## Checks

`docs_house_style` (the sweep's checks, #34) runs on this repository first.
