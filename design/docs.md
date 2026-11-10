<!-- SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer) -->
<!-- SPDX-License-Identifier: CC-BY-4.0 -->

# House style for docs

Standard version 1.0.0. The style the family's READMEs already share, written down so the sweep (#34) can check it.

## README order

1. The logo, centered.
2. A one-line bold promise and an italic second line.
3. Badges: gray labels for facts (`#6b7280`), `ember` for identity (`#f97316`), green for the cheeky stat (`#2ea44f`: "nicotine 0 mg", "glue scripts: 0"). The colors are in [generated/badges.json](generated/badges.json).
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
- Rounded boxes with gray outlines, `ember` arrows and highlights, mono labels, as `burn-model.svg`, `chain.svg`, `stages.svg` and `glue-vs-chain.svg` are.
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

`docs_house_style` (`xtask/tests/docs_house_style.rs`) is the sweep's check of everything above, and it runs on this repository first. For `docs/`, `design/` and the top-level pages it checks:

| check | what it asserts |
|---|---|
| license header | the CC-BY-4.0 header, or `REUSE.toml` maps the page to CC-BY-4.0 |
| one H1 | exactly one (the README's title is its logo) |
| sentence case | no title-case words after the first in a heading, apart from product names |
| code blocks | every fence names its language |
| images | no raster screenshots of text; only the social preview is a PNG |
| American English | no `colour`, `behaviour`, `licence`, `grey` and the like outside code |
| README order | the logo first and centered, a bold promise and an italic line, then tour, what you get, install, commands, limits, part of, licensing, in that order |
| repository files | the list above exists, `YANKED` once there is a release, and the social preview is 1280 by 640 |
