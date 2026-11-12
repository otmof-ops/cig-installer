#!/bin/sh
# SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
# SPDX-License-Identifier: FSL-1.1-ALv2
# The upgrade matrix (#21, carton 0.2.0 row 14): on each distro's container, install
# carton 0.1.0 as a user would, update to the candidate carton, prove status, every shim and
# doctor, then roll back and go forward again. The static ciginstaller and cigi-shim and the
# mirror come in <dist>. POSIX sh only (Alpine has no bash). Everything happens under a
# scratch directory with its own HOME and CIGSCRIPT_HOME; nothing outside it is touched.
#
# When a real carton 0.2.0 is published, this script and its workflow step change in three
# places:
#   1. BASE and CANDIDATE below are set from `manifest/products.toml` and the published
#      `manifest` release (expected: BASE=0.1.0, CANDIDATE=0.2.0).
#   2. CI's mirror is built with the real carton members, not the fixture candidate:
#      `cargo xtask fixture-mirror dist/mirror` with no `--candidate` once products.toml names
#      both cartons, or the real signed manifest is fetched into the mirror.
#   3. The `allow_single_source` opt-in goes away: a real carton comes from two sources
#      (the published manifest and GitHub releases), so the single-source refusal does not
#      apply.
#
# usage: ci/upgrade.sh <dist>   (<dist> holds ciginstaller, cigi-shim and mirror/)
set -eu

dist="${1:?usage: upgrade.sh <dist>}"
for f in ciginstaller cigi-shim mirror/manifest-stable.toml; do
  [ -f "$dist/$f" ] || { echo "::error::$dist/$f is missing"; exit 1; }
done

# BASE is the published carton 0.1.0. CANDIDATE is the next carton; until carton 0.2.0 is
# tagged, its members come from the fixture mirror (see the note at the top). The members
# are plain semver because ciginstaller's Pin parser accepts `X.Y` and `X.Y.Z` only.
BASE="${CIGI_UPGRADE_BASE:-0.1.0}"
CANDIDATE="${CIGI_UPGRADE_CANDIDATE:-0.2.0}"
BASE_CIG="${CIGI_UPGRADE_BASE_CIG:-1.20.0}"
BASE_CIGO="${CIGI_UPGRADE_BASE_CIGO:-1.16.0}"
CAND_CIG="${CIGI_UPGRADE_CAND_CIG:-1.21.0}"
CAND_CIGO="${CIGI_UPGRADE_CAND_CIGO:-1.17.0}"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/bin" "$work/home"
cp "$dist/ciginstaller" "$dist/cigi-shim" "$work/bin/"
chmod 755 "$work/bin/ciginstaller" "$work/bin/cigi-shim"
cp -R "$dist/mirror" "$work/mirror"

export HOME="$work/home"
export CIGSCRIPT_HOME="$work/home/.cigscript"
# The mirror is one source; the opt-in is `allow_single_source = true`, as the cargo tests do.
# `update` reads the mirror from `CIGI_MIRROR` (or the `mirror` setting); we set the env var so
# every command in this script talks to the same mirror the first install came from.
export CIGI_MIRROR="file://$work/mirror"
export NO_COLOR=1
umask 022
ci="$work/bin/ciginstaller"

step() { echo "== $*"; }
die() { echo "::error::$*"; exit 1; }

step "the machine"
( . /etc/os-release 2> /dev/null && echo "$PRETTY_NAME" ) || uname -a
uname -m
"$ci" --version

step "allow the fixture mirror (one source), the way a user does on a private mirror"
"$ci" config set allow_single_source true

# ---- 1. install carton $BASE ------------------------------------------------
step "install carton $BASE --yes --from <mirror>"
"$ci" install carton "$BASE" --yes --from "$work/mirror"

step "status after the base install"
"$ci" status | tee "$work/status-base.txt"
grep -q "cig $BASE_CIG" "$work/status-base.txt" || die "status does not show cig $BASE_CIG"
grep -q "cigo $BASE_CIGO" "$work/status-base.txt" || die "status does not show cigo $BASE_CIGO"

step "every shim on the base: cig $BASE_CIG, cigo $BASE_CIGO"
PATH="$CIGSCRIPT_HOME/bin:$PATH"
export PATH
# Each shim's `--version` starts with the product's name and version. cigo's fake prints an
# extra line about the cig it uses; we only check the first word-pair of the first line.
head1() { awk 'NR==1{print $1" "$2}'; }
got="$(cig --version | head1)"
[ "$got" = "cig $BASE_CIG" ] || die "cig shim printed '$got', not 'cig $BASE_CIG'"
got="$(cigo --version | head1)"
[ "$got" = "cigo $BASE_CIGO" ] || die "cigo shim printed '$got', not 'cigo $BASE_CIGO'"

step "doctor on the base finds nothing wrong"
"$ci" doctor

# ---- 2. update to the candidate --------------------------------------------
# `update --to <carton>` takes the carton over the current machine's whole set, through the
# trial, and switches at once. `--no-trial` skips the trial: there is no project here.
step "update --to $CANDIDATE --no-trial (the upgrade)"
"$ci" update --to "$CANDIDATE" --no-trial

step "status after the upgrade"
"$ci" status | tee "$work/status-cand.txt"
grep -q "cig $CAND_CIG" "$work/status-cand.txt" || die "status does not show cig $CAND_CIG after the upgrade"
grep -q "cigo $CAND_CIGO" "$work/status-cand.txt" || die "status does not show cigo $CAND_CIGO after the upgrade"

step "every shim on the candidate: cig $CAND_CIG, cigo $CAND_CIGO"
got="$(cig --version | head1)"
[ "$got" = "cig $CAND_CIG" ] || die "cig shim printed '$got', not 'cig $CAND_CIG' after the upgrade"
got="$(cigo --version | head1)"
[ "$got" = "cigo $CAND_CIGO" ] || die "cigo shim printed '$got', not 'cigo $CAND_CIGO' after the upgrade"

step "doctor on the candidate finds nothing wrong"
"$ci" doctor

# ---- 3. rollback to the base -----------------------------------------------
step "rollback to carton $BASE"
"$ci" rollback

step "status after the rollback"
"$ci" status | tee "$work/status-rollback.txt"
grep -q "cig $BASE_CIG" "$work/status-rollback.txt" || die "status does not show cig $BASE_CIG after the rollback"
grep -q "cigo $BASE_CIGO" "$work/status-rollback.txt" || die "status does not show cigo $BASE_CIGO after the rollback"

step "every shim on the base again"
got="$(cig --version | head1)"
[ "$got" = "cig $BASE_CIG" ] || die "cig shim printed '$got' after the rollback, not 'cig $BASE_CIG'"
got="$(cigo --version | head1)"
[ "$got" = "cigo $BASE_CIGO" ] || die "cigo shim printed '$got' after the rollback, not 'cigo $BASE_CIGO'"

step "doctor after the rollback"
"$ci" doctor

# ---- 4. forward again ------------------------------------------------------
step "update --to $CANDIDATE --no-trial (forward from the rollback)"
"$ci" update --to "$CANDIDATE" --no-trial

step "status after going forward"
"$ci" status | tee "$work/status-forward.txt"
grep -q "cig $CAND_CIG" "$work/status-forward.txt" || die "status does not show cig $CAND_CIG after going forward"
grep -q "cigo $CAND_CIGO" "$work/status-forward.txt" || die "status does not show cigo $CAND_CIGO after going forward"

step "every shim on the candidate again"
got="$(cig --version | head1)"
[ "$got" = "cig $CAND_CIG" ] || die "cig shim printed '$got' after going forward, not 'cig $CAND_CIG'"
got="$(cigo --version | head1)"
[ "$got" = "cigo $CAND_CIGO" ] || die "cigo shim printed '$got' after going forward, not 'cigo $CAND_CIGO'"

step "doctor after going forward"
"$ci" doctor

echo "upgrade $BASE -> $CANDIDATE, rollback, forward: ok"
