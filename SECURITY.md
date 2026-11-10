<!-- SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer) -->
<!-- SPDX-License-Identifier: CC-BY-4.0 -->
# Security policy

## Supported versions

Fixes go into the newest code on `main` until ciginstaller has releases; after
that, into the newest release.

## Reporting a vulnerability

Use GitHub's private vulnerability reporting on this repository (**Security
→ Report a vulnerability**). Do not open a public issue for a security
problem. Include the commit or version, the platform, and the command that
shows the problem. You'll get an acknowledgement within a few days and a fix
or a mitigation before anything is disclosed.

## What ciginstaller promises

An installer decides what runs on your machine, so it is held to more than
the rest of the family:

- **Nothing runs that was not verified.** Every build it installs is admitted
  at one of the three levels below before it is unpacked or run, and nothing
  that reaches none of them is installed. A way to make it install or run
  anything else is a security bug.
- **An install is a burn.** It is journaled and can be rolled back, and a
  failed install leaves the machine as it was.
- **No telemetry, ever.** It reports nothing about you or your machine to
  anyone unless you send a report yourself. An update check is a plain `GET`
  of public metadata with no identifier in it, and `update_check = off` stops
  even that. A test records the requests an install makes and fails if any
  carries an identifier.

## What is verified today

Verification today proves **integrity**: the bytes you got are the bytes the
maintainers published. It does not prove **authorship**. Nothing is signed,
because there are no signing keys yet.

Every artifact (a build of `cig`, `cigo`, `cigcompile` or ciginstaller itself)
is admitted at the highest level it reaches:

| level | what must be true |
|---|---|
| `pinned` | its SHA-256 equals an entry compiled into this ciginstaller build. The compiled-in list is empty today, so no build reaches this level yet |
| `attested` | `gh attestation verify` passes against the product's repository, and the digest equals the manifest's and the product release's own `.sha256` file. It applies only where `gh` is on `PATH`; without it, a build stays at `checksummed` |
| `checksummed` | the downloaded bytes, the digest in the manifest and the digest in the product release's own `.sha256` file all agree. The manifest is published from this repository and the `.sha256` file from the product's, so two sources agree |

Around those levels:

- **A floor.** The `trust` setting (default `checksummed`) is the lowest level
  an artifact may reach, and `install --require <level>` raises it for one
  run. Nothing lowers it.
- **The manifest is admitted before anything it names.** It must parse, must
  not have expired, must match its own `.sha256` file, and its `sequence` must
  not be lower than the highest the installer has seen for that channel, which
  stops an old manifest being replayed. The first manifest a machine ever
  fetches is trusted on first use, and its `sequence` is recorded from then on.
- **A mirror is one source.** A mirror directory serves both the manifest and
  the builds, so the two sources are not independent. The receipt says so, and
  the doctor says so.
- **Downloads are plain and narrow.** One HTTPS client, `GET` only, with
  `User-Agent: ciginstaller`. Redirects stay on the same host or go to GitHub's
  release host, never from HTTPS to HTTP, and stop after five. It downloads the
  exact release URLs the manifest names and never calls the GitHub API.
- **Failure leaves the machine as it was.** Files land through the
  installer's journal and switch in with one `rename`. A kill before it rolls
  back; a kill after it rolls forward.

## What is not verified yet

- **No signed metadata.** [The Update Framework (#9)](https://github.com/otmof-ops/cig-installer/issues/9)
  is not built. Until it is, nothing defends against an attacker who controls
  both this repository's `manifest` release and a product's release, a
  mirror that serves a stale manifest for as long as it has not expired, or
  metadata and builds that were each valid once and are mixed. The
  `sequence` check is a partial defense against rollback only.
- **No signature on the installer or its bootstrap script.** The installer's
  own tarballs are built by a workflow that publishes a checksum and a build
  attestation for each, and a draft release is the only thing it creates.
  Publishing one is a human act. `self update` admits the new build at the
  same levels as any other.
- **No pinned digests yet.** The `pinned` level exists and is tested, and the
  list it reads is empty until a manifest is reviewed and its digests are
  compiled in with `cargo xtask pins`.

Signed metadata plugs in behind one seam, so that nothing else in the
installer changes when it arrives; the decision is
[docs/decisions/0001-architecture.md](docs/decisions/0001-architecture.md),
section 7.
