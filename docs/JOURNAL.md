<!-- SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer) -->
<!-- SPDX-License-Identifier: CC-BY-4.0 -->

# The installer's journal

Every change the installer makes goes through one transaction, written ahead to a journal. The
format is a contract: `ciginstaller rollback` reads it, and so does `cig runs adopt`, which turns a
committed transaction into a cig run that `cig unburn` can restore (#7).

## Where it is

`<home>/installer/txn/<id>/journal.jsonl`, one JSON object per line, beside the copies it saved:
`<home>/installer/txn/<id>/saved/`. A committed transaction is kept for 7 days
(`TXN_RETENTION_DAYS`); after that there is nothing to adopt or roll back.

## A line

Each line is a sequence number and a record, flat: `{"seq": 0, "t": "begin", ...}`. `seq` counts
from 0 with no gaps, and `t` is one of `begin`, `mkdir`, `store`, `link`, `move`, `write`,
`delete`, `block`, `flip`, `commit` and `rollback-done`.

| `t` | fields | what it did |
|---|---|---|
| `begin` | `label`, `pid`, `format` | opened the transaction (`format` is the journal's version) |
| `mkdir` | `path`, `before` | made a directory |
| `store` | `object`, `exec`, `before` | put an object into the shared store (`humidor/store/objects`) |
| `link` | `from`, `to`, `before` | made a hard or symbolic link |
| `move` | `from`, `to`, `before` | renamed `from` to `to` |
| `write` | `path`, `before`, `after_sha256` | wrote a file, whose new content has `after_sha256` |
| `delete` | `path`, `before` | removed a file, a directory or a link |
| `block` | `path`, `before`, `marker` | wrote the marked block into a file outside the home (a shell profile) |
| `flip` | `index_sha256`, `previous_sha256`, `saved` | replaced `installer/shim-index` at once; `saved` is the previous index |
| `commit` | `recovered` | the transaction committed (`recovered` when recovery finished it) |
| `rollback-done` | | the transaction was rolled back |

## `before`

What a path looked like before the op, tagged by `state`:

| `state` | fields | |
|---|---|---|
| `absent` | | nothing stood there |
| `file` | `saved`, `sha256` | a file; `saved` is its copy under the transaction (absent for `store`, where it only means the object was already there) |
| `dir` | `saved` | a directory; `saved` is where `delete` moved it |
| `link` | `target` | a symbolic link, never followed; a rollback makes the same link again |

## Undoing an update with cig

```sh
# close ciginstaller first: cig does not take its lock
cig runs adopt <home>/installer/txn/<id>   # prints the run id
cig unburn <run id>
```

`cig runs adopt` checks the commit line, every saved file's sha256, and that every path stays inside
the home. A transaction that wrote outside the home (a `block` in a shell profile) is refused whole
and stays `ciginstaller rollback`'s.
