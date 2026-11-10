# Contributing

The plan of record is the issues. Each says why, what lands, and when it is
done. Pick one, or open one for what you have in mind before writing much.
Decisions are recorded in `docs/decisions/` as they are made.

## Building and testing

```sh
cargo build
cargo test
```

CI runs formatting, clippy and the tests.

## Pull requests

One subject per pull request, naming its issue (`Closes #5`), with the test
that proves it. Reviews are about the code and the behavior; expect a reply,
and expect it to be kind.

## Licensing your contribution

Code is FSL-1.1-ALv2 and documentation is CC BY 4.0; [REUSE.toml](REUSE.toml)
maps every path, and every source file carries SPDX headers. A new source file
gets the same two lines as its neighbors, with the code identifier:

```
SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
SPDX-License-Identifier: FSL-1.1-ALv2
```

Documentation files use `CC-BY-4.0` in the second line. See [LICENSING.md](LICENSING.md)
for which versions carry which license.

## The contributor agreement

ciginstaller uses a Contributor License Agreement, not a DCO. Before a pull
request from you can be merged, you sign the agreement once:

- individuals: [`.github/CLA/ICLA.md`](.github/CLA/ICLA.md)
- companies whose employees contribute: [`.github/CLA/CCLA.md`](.github/CLA/CCLA.md)

You keep your copyright. The agreement gives the project a license to use,
relicense and distribute your contribution, a patent license for what you
contribute, and your confirmation that the work is yours to give. It exists so
the project can, for example, offer a commercially licensed edition or move to
a newer license later without tracking down every past contributor. Signing is
a comment on your first pull request:

> I have read the CLA Document and I hereby sign the CLA

A bot records it; you will not be asked again.
