<!-- SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer) -->
<!-- SPDX-License-Identifier: CC-BY-4.0 -->

# 0002: License of ciginstaller

- Status: accepted
- Date: 2026-11-10
- Issue: otmof-ops/cig-installer#27

## Context

The repository went public on 2026-11-10 under Apache-2.0. Issue #27 sets out
the aim for the family: free to use, study and change, with one thing
prevented, which is reselling it. Its option 4 gives the example: cig and
CigCompile stay Apache-2.0 for adoption, while ciginstaller, the guard's higher
tiers, CigarScript and CIG.OS take the restrictive license. #27 planned the change
for carton 1.0.0 (#25). ciginstaller has no release yet, so there is nothing
to wait for and every commit under Apache-2.0 widens what cannot be taken back.

## Decision

From the licensing commit on, the code is under **FSL-1.1-ALv2**, the
Functional Source License 1.1 with an Apache-2.0 future license. Each version
converts to Apache-2.0 on the second anniversary of the date it is made
available. Documentation stays CC-BY-4.0. The copyright line is
`Copyright 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)`.

Contributions come in under the contributor agreement (ICLA and CCLA, the same
as CigScript's with the project name changed); the DCO is dropped.
[LICENSING.md](../../LICENSING.md) holds the table of which versions carry
which license.

## Options considered

| License | Why not |
|---|---|
| Apache-2.0 (unchanged) | Allows resale, which is the thing #27 wants to prevent. |
| BUSL-1.1 | Needs an Additional Use Grant written by the licensor, and that grant is where "resale" and the user's own products would have to be defined. Wording that has to be right and cannot be borrowed. |
| Elastic-2.0 | Bars hosted managed services but does not stop selling copies. |
| PolyForm Noncommercial 1.0.0 | Bars every commercial use, so a company could not use the installer internally. |
| PolyForm Small Business 1.0.0 | Bars organizations above its size limit from using it at all. |
| FSL-1.1-MIT | Same terms with MIT as the future license; Apache-2.0 matches the rest of the family and carries an explicit patent grant. |
| **FSL-1.1-ALv2** | Chosen. The text is fixed and unmodified, so there is no grant to write. It allows internal use, study, changes and sharing, bars selling it or offering it in a competing product or service, and converts each version to Apache-2.0 after two years. Liquibase made the same move, Apache-2.0 to FSL-1.1-ALv2, for its 5.0 line. |

## Consequences

- **Not open source.** The OSI does not recognize FSL. The README and
  LICENSING.md say source-available, and some organizations and distributions
  will not ship or depend on it.
- **The Apache era.** Commits `5571157` through `39a9b00` (31 commits,
  2026-11-10, no release) were public under Apache-2.0 for about twelve hours.
  That grant is irrevocable for any copy taken; none is recorded (zero forks,
  clones and views). The range is marked by the annotated tag
  `apache-2.0-last` on `39a9b00` and recorded in LICENSING.md.
- **Contributions.** A CLA lets the project relicense and convert contributions
  that a DCO would not. Signing is a one-time comment on a pull request,
  recorded by a bot. Nothing from outside has been merged, so nothing needs
  re-collecting.
- **The text.** LICENSE is the publisher's text (fsl.software, identical to the
  SPDX License List copy), with only its notice line filled in: the year and
  the licensor.
- **Trademarks.** FSL grants no trademark rights. Forks are possible under its
  terms but need their own name; TRADEMARKS.md says so.

## Still open from #27

- The family-wide review at carton 1.0.0: which license cig, cigo, CigCompile,
  cigar and CIG.OS carry, and how compiled programs are treated. This record
  decides ciginstaller only.
- `ciginstaller licences` and the first-run display of the terms, with the
  acceptance recorded locally and nothing sent anywhere. This is a feature
  still to build, separate from the license change.
