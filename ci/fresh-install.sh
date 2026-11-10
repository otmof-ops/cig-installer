#!/bin/sh
# SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
# SPDX-License-Identifier: FSL-1.1-ALv2
# A fresh install on a clean machine (#21), run inside a container by the `fresh-install` CI job:
# the static ciginstaller and cigi-shim, and the fixture mirror made by `cargo xtask fixture-mirror`,
# in <dist>. POSIX sh only: Alpine has no bash. Everything happens under a scratch directory with
# its own HOME and CIGSCRIPT_HOME; nothing outside it is touched.
# usage: ci/fresh-install.sh <dist>   (<dist> holds ciginstaller, cigi-shim and mirror/)
set -eu

dist="${1:?usage: fresh-install.sh <dist>}"
for f in ciginstaller cigi-shim mirror/manifest-stable.toml; do
  [ -f "$dist/$f" ] || { echo "::error::$dist/$f is missing"; exit 1; }
done

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/bin" "$work/home"
# a downloaded artifact has lost its execute bits, and a read-only mount cannot get them back
cp "$dist/ciginstaller" "$dist/cigi-shim" "$work/bin/"
chmod 755 "$work/bin/ciginstaller" "$work/bin/cigi-shim"
cp -R "$dist/mirror" "$work/mirror"

export HOME="$work/home"
export CIGSCRIPT_HOME="$work/home/.cigscript"
export NO_COLOR=1
umask 022
ci="$work/bin/ciginstaller"

step() { echo "== $*"; }
die() { echo "::error::$*"; exit 1; }

step "the machine"
( . /etc/os-release 2> /dev/null && echo "$PRETTY_NAME" ) || uname -a
uname -m

step "ciginstaller --version"
"$ci" --version

step "a mirror is one source, which is refused until the user allows it"
if "$ci" install carton --yes --from "$work/mirror"; then
  die "a single-source mirror was accepted without allow_single_source"
fi
if [ -n "$(find "$CIGSCRIPT_HOME/toolchains" -type f 2> /dev/null | head -n 1)" ]; then
  die "the refused install left a toolchain behind"
fi

step "allow it, as a user would (the fixture mirror is one operator's, not two sources)"
"$ci" config set allow_single_source true

step "install carton --yes --from <mirror>"
"$ci" install carton --yes --from "$work/mirror"

step "status"
"$ci" status | tee "$work/status.txt"
grep -q "cig 1.20.0" "$work/status.txt" || die "status does not show cig 1.20.0"

step "a shim runs the installed tool (bin/ on PATH, as the installer says to do)"
PATH="$CIGSCRIPT_HOME/bin:$PATH"
export PATH
got="$("$CIGSCRIPT_HOME/bin/cig" --version)"
echo "$got"
[ "$got" = "cig 1.20.0" ] || die "the cig shim printed '$got', not 'cig 1.20.0'"

step "doctor finds nothing wrong"
"$ci" doctor

echo "fresh install ok"
