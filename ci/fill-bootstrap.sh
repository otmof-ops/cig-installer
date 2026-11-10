#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
# SPDX-License-Identifier: FSL-1.1-ALv2
# Fills the bootstrap templates (packaging/bootstrap/install.sh.in, install.ps1.in) with one
# release's tag, repository and per-platform tarball SHA-256s, so the published script carries
# the digests it checks. Writes install.sh, install.ps1 and a .sha256 beside each.
# A platform with no tarball in the directory is written as `none`, and the script then refuses
# it by name. Fails if no Linux tarball is there, if any sidecar is malformed, or if a
# placeholder is left over.
# usage: ci/fill-bootstrap.sh <tag vX.Y.Z> <owner/repo> <directory of tarballs and .sha256 files> <output directory>
set -euo pipefail
tag="${1:?usage: fill-bootstrap.sh <tag> <owner/repo> <assets dir> <out dir>}"
repo="${2:?missing owner/repo}"
assets="${3:?missing assets directory}"
out="${4:?missing output directory}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

[[ "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$ ]] || { echo "::error::tag '$tag' is not vX.Y.Z"; exit 1; }
[[ "$repo" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]] || { echo "::error::repository '$repo' is not owner/name"; exit 1; }
ver="${tag#v}"

# the digest of ciginstaller-v<ver>-<platform>.<ext>, from its sha256sum-format sidecar, or `none`
digest_of() {
  local platform="$1" ext="$2" name side line
  name="ciginstaller-v${ver}-${platform}.${ext}"
  side="$assets/$name.sha256"
  if [[ ! -f "$side" ]]; then echo none; return; fi
  [[ "$(wc -l < "$side")" == 1 ]] || { echo "::error::$side is not exactly one line" >&2; exit 1; }
  line="$(cat "$side")"
  [[ "$line" =~ ^([0-9a-f]{64})[[:space:]]+[\*]?${name//./\\.}$ ]] \
    || { echo "::error::$side is not a sha256sum line for $name" >&2; exit 1; }
  echo "${BASH_REMATCH[1]}"
}

lx="$(digest_of linux-x86_64 tar.gz)"
la="$(digest_of linux-aarch64 tar.gz)"
wx="$(digest_of windows-x86_64 zip)"
if [[ "$lx" == none || "$la" == none ]]; then
  echo "::error::both Linux tarballs and their .sha256 files are needed in $assets"
  exit 1
fi

mkdir -p "$out"
for t in install.sh install.ps1; do
  sed -e "s|@VERSION@|$ver|g" \
      -e "s|@TAG@|$tag|g" \
      -e "s|@REPO@|$repo|g" \
      -e "s|@SHA256_LINUX_X86_64@|$lx|g" \
      -e "s|@SHA256_LINUX_AARCH64@|$la|g" \
      -e "s|@SHA256_WINDOWS_X86_64@|$wx|g" \
      "$here/packaging/bootstrap/$t.in" > "$out/$t"
  if grep -nE '@[A-Z0-9_]+@' "$out/$t"; then
    echo "::error::$t still holds a placeholder"
    exit 1
  fi
  (cd "$out" && sha256sum "$t" > "$t.sha256")
done
chmod +x "$out/install.sh"
sh -n "$out/install.sh"
echo "filled $tag: linux-x86_64 $lx, linux-aarch64 $la, windows-x86_64 $wx"
