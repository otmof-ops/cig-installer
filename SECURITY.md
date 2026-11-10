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
  carries an identifier. A staged rollout (`rollout = N%`) is decided on the
  machine from a hash of its machine id with a fixed prefix; neither the id nor
  the hash is ever printed, stored or sent. `CIGI_TEST_MACHINE_ID` replaces the
  id for one run, for tests, and can only move that machine's own place in the
  rollout.
- **A crash report is yours to send.** A crash is written to the local ashtray
  and nothing leaves until you run `ciginstaller crash send`; `reports = never`
  refuses even that. What travels is the allowlist in
  `schemas/report.schema.json`, printed by `crash show` first: your paths are
  shapes, your user and host names are removed, no file contents or environment
  values are included. A report that carries one is a security bug.
- **A bug-report bundle is yours to read and send.** `ciginstaller report` writes
  and sends nothing on its own. `--out FILE` writes a bundle (mode 0600) and
  `--send` files it through the same route and the same `reports` setting as a
  crash. Every string in it is anonymised once, before either, so the file you
  read is what is sent; it holds versions, the operation's journal and the
  trial's differences, never file contents, environment values, user or host
  names. cig builds its own bundle for a cig run, and the installer runs it
  only from a build whose files match their receipt.
- **A bug report is yours to read, keep and send.** `ciginstaller bug` shows
  exactly what would travel, section by section (the version matrix always, the
  rest removable), and saves it only on your yes (the last question or
  `--save`); it sends only on `--send` or "send it", through the crash route
  and its `reports` setting. The words you type stay yours, but a span shaped
  like a path, an address, an e-mail address or your name is marked and asked
  about first, and a token-shaped span is always replaced; the spans you keep
  are listed in what you saw and are the only part of the free text that is not
  anonymised again on its way out. `--yes` answers none of the questions, and
  no request is made before a yes.

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
- **A mirror is one source, and so is a bundle.** A mirror directory serves
  both the manifest and the builds, so the two sources are not independent. A
  bundle (`install --from <file.tar>`) is the same thing in one file: the
  manifest, the checksums and the builds all come out of it, so it is treated
  exactly as a mirror is. It reaches `pinned` only for builds whose digests are
  compiled into this installer, and otherwise is refused (E1822) unless
  `allow_single_source` is set. Nothing about a bundle lowers the `trust`
  floor, and no flag, variable or file in it can. The receipt says so, and the
  doctor says so. **Bundles are unsigned until #9:** anyone who can write the
  file can write a manifest, its checksum and builds that agree with each
  other, which is why the digests compiled into the installer are what count.
  Unpacking refuses anything but plain files with plain names, in a private
  directory inside the home that is removed afterwards, and every build is
  checked against the manifest inside the bundle before it is used. `mirror`
  admits every build before it writes it, writes through a partial file and a
  rename, puts the manifest in last, never removes a file, and refuses to write
  a manifest older than the one already in the directory.
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
  Publishing one is a human act. The same workflow fills `install.sh` and
  `install.ps1` from their templates with that release's tarball digests and
  attaches them, with a checksum and an attestation each. There is no
  installer release yet, so no such script has been published.
  The script checks the downloaded tarball's SHA-256 against the digest it
  carries and runs `gh attestation verify` when `gh` is present; it refuses on
  any mismatch and leaves nothing behind. That is integrity, not authorship: a
  script that was swapped before you downloaded it carries the digests of
  whatever its maker chose, which is why you read it first and check it
  against the `.sha256` and the attestation on the release page. It checks no
  signature, because there are none until #9 is built. It never pipes anything
  into a shell and sends nothing about the machine, only plain `GET`s of the
  release's tarball. `CIGI_BOOTSTRAP_BASE_URL` exists for mirrors and tests and
  says so when used; the embedded digest still has to match. The attestation is
  checked whenever `gh` is installed and signed in, and nothing turns it off. `self update` admits the new build at the
  same levels as any other.
- **No pinned digests yet.** The `pinned` level exists and is tested, and the
  list it reads is empty until a manifest is reviewed and its digests are
  compiled in with `cargo xtask pins`.

Signed metadata plugs in behind one seam, so that nothing else in the
installer changes when it arrives; the decision is
[docs/decisions/0001-architecture.md](docs/decisions/0001-architecture.md),
section 7.
