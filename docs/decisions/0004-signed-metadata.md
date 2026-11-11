<!-- SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer) -->
<!-- SPDX-License-Identifier: CC-BY-4.0 -->

# 0004: Signed metadata (TUF) for what the installer fetches

- Status: accepted (2026-11-11)
- Date: 2026-11-10
- Issues: otmof-ops/cig-installer#9
- Evidence: `spikes/tough-eval` (22 tests, run with `cargo test` inside it); the same attacks against
  the installer's own client in `xtask/tests/tuf_attacks.rs` and `xtask/tests/tuf_http.rs`

## Context

Decision 0001 section 7 admits an artifact at `pinned`, `attested` or `checksummed`, and says
TUF enters through the `Trust` trait. SECURITY.md lists what that leaves open: an attacker who
controls both this repository's `manifest` release and a product's release; a mirror that serves a
stale manifest until it expires; metadata and builds that were each valid once and are mixed. The
manifest's `sequence` is a partial rollback defense only. The installer's own tarballs and the
bootstrap scripts carry checksums and attestations and no signature.

Threats, in the order the controls below answer them (assume the host serving the files is
breached):

1. A build or manifest is replaced (tamper, bad mirror).
2. Old, once-valid metadata is replayed (rollback).
3. Stale metadata is served for as long as it has not expired (freeze).
4. Valid files from different moments are combined (mix and match).
5. One stolen online key, or one product pipeline, is used to publish beyond its remit.
6. A signing key is lost or must be revoked.

## Decision

Adopt TUF 1.0 for the manifest, every product artifact, the installer's own tarballs and the
bootstrap scripts. The client is the `tough` crate, used for metadata only (section 9).

### 1. Roles

One repository, one root. Products are delegations, not separate repositories.

| role | signs | keys | threshold | held |
|---|---|---|---|---|
| `root` | the key and threshold of every role | 3 | 2 | offline |
| `targets` | the delegations, once (and again only when a delegation changes or the sitting comes round) | 1 | 1 | offline, not a root key |
| `releases` (delegated from `targets`, section 1a) | the manifest, every build it names, the installer's tarballs and the bootstrap scripts | 1 | 1 | online |
| `cig`, `cigo`, `cigcompile`, `cigar` (delegated from `targets`, later) | that product's own artifact entries, when a product's pipeline signs for itself | 1 each | 1 | that product's release pipeline |
| `snapshot` | the version and hash of `targets` and every delegated role | 1 | 1 | online |
| `timestamp` | the version and hash of `snapshot` | 1 | 1 | online |

- Key type is Ed25519 throughout (what `tough` generates, loads and verifies; the spike uses it).
- A delegation's paths are `<tool>-[0-9]*` and it is `terminating`, so `cig` can never name a
  `cigo-` file and a future `cig-foo` product cannot be captured by `cig`. The spike proves a
  delegated key is refused outside its paths when the repository is built
  (`a_pipeline_key_cannot_sign_another_products_file`).
- A target's name is `<tool>-<version>-<target>`, the key `pins.rs` already uses. Its signed
  `custom` field carries the build attestation reference (`repo`, workflow, subject), and the
  installer checks it at install exactly as `Attest` does today. The spike reads it back
  (`signed_targets_carry_length_digest_and_attestation_reference`).
- The manifest is signed by the online `releases` role (section 1a), not by the offline `targets`
  role as first proposed. A carton is still a human act (Jay reads the assembly and starts the
  signing workflow), but no offline key is needed to cut one.
- A compromised product pipeline can publish builds of that product and nothing else. It cannot
  change the manifest, so a build only installs if a carton names it; it can still publish extra
  builds that no manifest points at, which are inert. (Until a product signs for itself, its
  entries are written into `releases` from the manifest, so this applies from the day a product
  gets its own delegation.)

### 1a. The online delegation for releases

The offline `targets` key signs, once, a delegation to a role named `releases`, held by an online
key (`TUF_RELEASES_KEY`, a secret of the protected `tuf-online` environment). `releases` is
`terminating` and may sign only these paths (`xtask::tuf::RELEASE_PATHS`):
`manifest-*.toml`, `cig-*`, `cigo-*`, `cigcompile-*`, `cigar-*`, `ciginstaller-*`, `install.sh`,
`install.ps1`. A release then needs only online keys: `releases` signs the new manifest and its
build entries, `snapshot` and `timestamp` follow. A new product name is a new path, which only the
offline key can add (`tuf sign targets` after editing the list), so widening the remit is always an
offline act.

What this gives up, said plainly. Under the first draft a stolen online key could never name a
build; now a stolen `releases` key can publish a manifest and builds inside those paths until the
delegation is rotated (`tuf rotate releases`, offline key, then root-signed snapshot and timestamp
follow). It cannot touch the root, the delegation, or any path outside the list; the client
refuses a `releases` file that names one (`a_releases_key_cannot_vouch_for_a_name_outside_its_paths`),
and a forged `releases` role signed by anyone else is refused even when the snapshot and timestamp
keys are also stolen (`stolen_snapshot_and_timestamp_keys_cannot_publish_a_build`). What still
stops a stolen `releases` key from installing something new on its own: a compiled-in pin that
disagrees with a signed digest is a refusal, not a downgrade (section 5); the manifest `sequence`
cannot go backwards; a published name never changes its bytes (`xtask` refuses to re-sign one);
and every build's signed attestation reference must match the manifest, and `gh attestation
verify` runs when `gh` is present. The compromise is bounded by those and by the 30-day life of the
`releases` metadata, and detected by anyone who compares the `tuf` release with the `manifest`
release.

### 2. Expiries

| role | expires after | re-signed |
|---|---|---|
| `root` | 365 days | every 180 days, key change or not |
| `targets` | 365 days | with the root, every 180 days (it signs only the delegation now, so no carton needs it) |
| `releases` | 30 days | with each release, and by the daily online job so it never lapses |
| other delegated roles | 365 days | with each release of that product; a warning at 30 days left |
| `snapshot` | 30 days | with every change to any metadata, and weekly |
| `timestamp` | 7 days | daily, by the online job |

Reasons: the freeze window an attacker can hold a client in is the shortest expiry on the path, so
it is the `timestamp` (7 days). It is 7 and not 1 because the online job is a scheduled workflow
and GitHub disables scheduled workflows on a repository that has been quiet for 60 days; six missed
runs of slack stops one outage freezing every client. The `root` is long because an expired root
cannot be replaced by the mechanism that would replace it. The `targets` role is long for the same reason as
the root: the offline key is touched twice a year, at one sitting. The manifest's own `expires`
still obeys `MAX_EXPIRES_AHEAD_SECS` (90 days).

### 3. Key custody

Never in the repository and never in a workflow log (Jay's decision 3, 2026-11-11). The keys are
generated on the maintainer's build machine by `cargo xtask tuf keygen`, to files outside the
repository, which `xtask` refuses to read from inside it.

- Root keys: three, each exported encrypted to its own medium and then removed from the build
  machine; Jay moves them offline and keeps them in different places. Two are needed to change the
  root.
- Targets key: made at the same sitting, exported the same way, used only for the delegation and the
  twice-yearly re-signing.
- Snapshot, timestamp and releases keys: three keys, held as secrets (`TUF_SNAPSHOT_KEY`,
  `TUF_TIMESTAMP_KEY`, `TUF_RELEASES_KEY`, base64 of the PKCS#8 key, which `keygen --base64`
  writes) of the protected GitHub environment `tuf-online`, which only the signing workflow on
  `main` can read. Theft of snapshot and timestamp yields freeze and replay up to the expiries
  above and nothing that verifies as a build (`stolen_snapshot_and_timestamp_keys_cannot_publish_a_build`);
  theft of `releases` is bounded as section 1a says.
- Pipeline keys (later): one per product, a secret of that product's release workflow, never shared.
- The CigarScript package signature and the blacklist bundle (decisions 20 and 23) use the same
  `targets` key through the same `tuf sign` commands; their file names are added to the delegation
  paths at a sitting.

### 4. Rotation, and the rehearsal

Every key can be replaced without a new installer, because a client follows `N.root.json` from the
root it shipped with up to the latest (tested: `rotating_the_timestamp_key_and_a_lost_root_key_is_followed_by_an_old_client`).

| event | action |
|---|---|
| routine, any online or pipeline key | new root version naming the new key; old key retired |
| lost root key | root v(N+1) replaces it, signed by two of the old keys and two of the new |
| stolen online or pipeline key | the same, at once, and the snapshot/timestamp job is re-run |
| stolen root key | rotate; with two stolen, see below |
| two root keys lost or stolen | no in-band recovery exists. A new installer release ships a new root, and the bootstrap scripts say so. This is the case the rehearsal exists to make rare |

A new root must be signed by the old threshold and by its own; a change with too few of either is
refused (`a_root_change_signed_by_too_few_old_keys_is_refused`,
`a_root_change_signed_by_too_few_new_keys_is_refused`), and a retired key signs nothing
afterwards (`a_retired_timestamp_key_is_refused_after_rotation`).

Rehearsal #9 asks for, by Jay: before the first public release, on a scratch repository with
throwaway keys, (1) rotate the timestamp key; (2) replace one root key as if lost; (3) cut a root
with two of three old keys and confirm an installer built with the old root follows it; (4) try
with one old key and confirm it refuses; (5) write down how long each step took and what was
unclear, into SECURITY.md. Repeat after any change of custody. The spike's rotation tests are the
automated part; the human part is the ceremony.

### 5. Trust levels and the pinned floor

`TrustLevel` gains `Signed` and the order becomes `Checksummed < Attested < Signed < Pinned`.

- `Signed`: the digest and size come from targets metadata that verified against the root compiled
  into this installer, within every expiry, at or above the stored version of every role. An
  artifact needs the `Signed` entry and the attestation reference to match the build.
- `Pinned` stays the top. A pin is a digest reviewed and compiled into this installer; it cannot be
  changed by any key, online or offline. A signed digest that disagrees with a pin is a refusal,
  not a downgrade.
- The default floor (`trust` in config) moves from `checksummed` to `signed` once the first signed
  carton is out. `checksummed` and `attested` remain for mirrors and offline use only on the
  explicit settings they need today.
- The rollback defense is the datastore, and it begins empty: a machine that has never seen
  version 5 accepts a correctly signed version 3 (`control_a_client_with_no_history_accepts_old_unexpired_metadata`).
  Closing that gap is the compiled-in floor: the installer build carries the lowest `snapshot` and
  `targets` version it will ever accept, written by `xtask` from the metadata Jay has just signed.
  `tough` has no such setting, so the check is the installer's, on `repo.snapshot()` and
  `repo.targets()` after load.
- The root compiled into the binary is the latest at build time. An old binary walks forward; it
  never needs the newest.
- The existing `sequence` stays in the manifest as a second, independent counter; the signed
  `targets` version and the manifest sequence must both not go backwards.

### 6. Expiry, `--offline` and the datastore

`tough` offers only `Safe` and `Unsafe`. Online runs always use `Safe`. The only caller of
`Unsafe` is `--offline` and bundle installs, which then enforce what they can themselves: the root
and targets expiries are checked by the installer from the loaded metadata, signature and version
checks still apply, and the output says which expiry was waived and how old the metadata is. The
clock is not trusted by the library alone: its datastore records the latest time it has seen and
refuses a clock stepped backward.

The datastore lives under `installer/` of the home, 0700, one per channel. Deleting it is a
rollback-defense reset and `doctor` says so.

### 7. Hosting and the consistent-snapshot seam

Metadata is published as assets of a rolling release (tag `tuf`) on this repository, using
consistent-snapshot names (`N.root.json`, `N.snapshot.json`, `N.targets.json`, `N.<role>.json`,
`timestamp.json`). Upload order is delegations and `targets`, then `snapshot`, then `timestamp`
last, so a reader never sees a timestamp that points at something absent. Products keep their
release pages; artifacts are not copied.

### 8. Bundles, mirrors and bootstrap scripts

- Mirrors serve the same files. A mirror cannot forge anything and can only withhold or serve old
  files, which expiry and the version floor bound. The "single operator source" rule for `checksummed`
  is moot at `Signed`: a mirror is as good as GitHub (`a_bad_mirror_*` tests).
- A bundle gains a `tuf/` directory with the metadata set and is admitted by the same code, with
  the `--offline` rules of section 6. `bundle.json` still trusts nothing.
- `install.sh` and `install.ps1` embed the installer's tarball digest today. After this change
  they embed the root and verify nothing themselves beyond the digest and `gh attestation verify`;
  the first `ciginstaller` run then verifies the signed metadata and, if the tarball it came from is
  not the signed `targets` entry, refuses and says so. The scripts themselves are signed targets
  too, so the instruction "read it, then check it against the signed entry" is one command
  (`ciginstaller verify-script <file>`). A script swapped before download is still a script the
  user must read: TUF cannot protect the first byte.
- `self update` fetches the installer's tarball as a `targets` entry and admits it at `Signed` or
  refuses.

### 9. Library choice and how it is used

Use `tough` 0.24 with `default-features = false`, for metadata only. The installer's own HTTP
client carries it through `tough`'s `Transport` trait (`UreqTransport`, 60 lines, tested against a
local server in `http_seam.rs`; it maps a 404 and only a 404 to "not found", which is what ends the
root chain). Artifacts are downloaded by the existing downloader and checked against the length and
SHA-256 of the signed entry, because `tough`'s own target fetch expects a hash-prefixed file name
and buffers a whole body, neither of which suits a release asset. Every `Err` from `tough` is a
refusal; the installer never branches on its variant (digest and length failures that happen while
a file streams come back as `Transport` errors, see the spike), and it maps `tough`'s 224-byte
error to its own `E18xx` code at the seam.

### 10. Migration from the checksummed manifest

1. Ceremony and rehearsal (Jay), section 4.
2. A `TufTrust` behind the `Trust` trait, with the attack tests written against it. `DigestTrust`
   unchanged. (Built: `crates/cigi-source/src/trust/tuf.rs`; the compiled-in root is
   `crates/cigi-source/tuf/root.json`, a placeholder until the ceremony; the floor is
   `crates/cigi-source/src/tuf_floor.rs`.)
3. `xtask manifest assemble --tuf-entries` also emits the signed-target entries; the online `tuf`
   workflow signs them with the `releases` key and then signs snapshot and timestamp. (Built.)
4. A carton is published with both the old checksummed manifest and the signed set. The installer
   prefers signed, and with the floor at `checksummed` still accepts the old.
5. After one release in which every path ran on signed metadata, the floor moves to `signed`, the
   old manifest sidecar path stays only for `--allow-unverified` fixtures, and SECURITY.md's "What
   is not verified yet" loses its first bullet.

## Evaluation of `tough` 0.24.0

What it stopped, against throwaway Ed25519 keys generated in each test (22 tests, all pass;
`cargo clippy --all-targets -- -D warnings` clean):

| attack | result |
|---|---|
| rollback: valid unexpired v1 after v2 was seen | refused, `OlderMetadata` |
| freeze: expired timestamp, snapshot or targets | refused, `ExpiredMetadata` (and loads only with `Unsafe`) |
| mix and match: old `targets` under new snapshot; old snapshot under new timestamp | refused, hash mismatch |
| bad mirror: one byte changed; longer; truncated; one signed target served as another | refused at read, hash or size |
| whole repository under other keys; stolen timestamp key alone | refused, verification fails on snapshot/targets |
| root change with too few old or new signatures; retired key after rotation | refused |
| delegated key signing outside its paths | refused when the repository is built |

Not shown by the spike and still to prove in the installer: a client-side refusal of a delegated
role that oversteps its paths (only the publishing side was exercised; `tough` states its own suite
covers it), and the clock-stepped-backward refusal. `tough`'s crate docs say delegated roles are
not supported; its source loads and verifies them and the spike's delegation test reads two products'
targets through it, so the docs are stale, but that is a maintainer's word against this evidence.

Maintenance (crates.io and GitHub, read 2026-11-10): crate `tough` 0.24.0 released 2026-07-10;
earlier 0.23.0 (2026-06-18), 0.22.0 (2026-04-24), 0.21.0 (2025-04-22); about 2.9 million downloads,
about 0.9 million recent; repository `awslabs/tough`, not archived, last push 2026-11-03, 69 open
issues, 235 stars; used by Bottlerocket. Pre-1.0: the minor version moves about every one to three
months, so the pin is exact and a bump is a reviewed change. It is async and needs tokio.

Size, release profile of the workspace (lto, one codegen unit, `strip = "debuginfo"`, x86_64
Linux glibc), measured with `spikes/tough-eval`:

| binary | bytes |
|---|---|
| `baseline`: `ureq` + rustls + SHA-256, what trust does today | 2,311,496 |
| `tuf-file`: `tough` alone, no HTTP | 5,846,896 |
| `tuf-client`: `tough` carried by `ureq` | 7,460,264 |

`tough` adds 5,148,768 bytes (about 4.9 MiB) over the HTTPS stack the installer already carries.
By symbol size (approximate) about 1.9 MiB of that is AWS-LC, the C crypto library behind
`aws-lc-rs`, which `tough` calls directly and which cannot be turned off. The installer is 2.9 MB
today against an 8 MB budget (0001, "Open"); this puts it at about 8.0 MB, at the limit, so the budget
decision is Jay's (section "What is Jay's"). The shim does not link `tough`.

Dependencies: the spike's normal graph is 113 crates including itself; 21 of those are not in the
installer's `Cargo.lock` (317 packages), and 4 more are build-time only. Both rustls providers are
linked (`ring` for `ureq`, `aws-lc-rs` for `tough`). Building `aws-lc-sys` needs a C compiler and
CMake on Linux and macOS and NASM on Windows x86-64 unless its prebuilt-NASM option is set; only
Linux was built here, so macOS, Windows and musl are unverified and must be tried before this is
adopted.

Licenses, as each crate declares them (`cargo license`), for what is new to the installer's lock:

| license expression | crates |
|---|---|
| `Apache-2.0 OR MIT` | tough, dyn-clone, futures, futures-channel, futures-executor, olpc-cjson, pin-project, pin-project-internal, portable-atomic, portable-atomic-util, serde_plain, snafu, snafu-derive, typed-path, cmake (build), jobserver (build) |
| `MIT` | tokio, tokio-util, pem, fs_extra (build) |
| `MIT OR Unlicense` | jiff, jiff-core |
| `Apache-2.0 OR MIT OR Zlib` | tinyvec |
| `(Apache-2.0 OR MIT) AND Unicode-3.0` | unicode-normalization |
| `(Apache-2.0 OR ISC) AND ISC` | aws-lc-rs |
| `(Apache-2.0 OR ISC OR MIT) AND (Apache-2.0 OR ISC OR MIT-0) AND (Apache-2.0 OR ISC) AND Apache-2.0 AND BSD-3-Clause AND ISC AND MIT` | aws-lc-sys |
| `Apache-2.0 OR CC0-1.0 OR MIT-0` | dunce (build) |

`tough` itself is `MIT OR Apache-2.0` (`LICENSE-MIT` and `LICENSE-APACHE` ship in the crate). The
installer's own license is FSL-1.1-ALv2 (decision 0002). None of the expressions above is a
copyleft grant without a permissive alternative, but whether permissive notices of this set are
compatible with FSL-1.1-ALv2 and what `THIRD-PARTY-NOTICES.md` must carry for `aws-lc-sys` (BSD-3-Clause,
ISC and others, bundled C source) is a license-gate question and is decided from the law store, not
here. `ci/gen-third-party.py` regenerates the notices from `cargo metadata` once `tough` is in the
workspace.

## Consequences

- One new trust level, one new key hierarchy, and a twice-yearly human duty (the root and `targets`
  re-signing, at one sitting) that did not exist.
- About 5 MB of binary, one C library in the build, and a pre-1.0 dependency pinned exactly.
- A loss of two root keys is recoverable only by a new installer release.
- The first byte of an install is still the user's to check; TUF protects every byte after it.

## Alternatives considered

- **Sign the manifest only (minisign or similar).** Smaller and no C library, and it stops
  tampering. It does not stop rollback, freeze or mix and match, and has no per-product
  delegation, which #9 asks for.
- **Write the TUF client ourselves.** About the same size as the verifying core of `tough`, and
  the bug surface the specification exists to avoid.
- **`sigstore-tuf` or another Rust TUF crate.** Aligned with Sigstore's own root rather than ours;
  not evaluated here.
- **Keep digests and attestations, add only `sequence`.** What exists. Listed in SECURITY.md as
  not enough.

## What the maintainer decides

- Decide the size budget: accept about 8 MB, or ask for a TUF client without aws-lc.
- The key ceremony: generate the three root keys, the targets key, the two online keys, the
  pipeline keys; choose and hold the media. Nothing in this repository generates or contains them.
- Run the rehearsal in section 4 before the first signed carton, and record it.
- Approve the cadences (section 2) and the scheduled signing workflow, including that the online
  keys live in a GitHub environment secret.
- Give the word for the `tuf` rolling release and the floor change in section 10 step 5.
- The license-gate decision on the new dependencies.
