#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
# SPDX-License-Identifier: FSL-1.1-ALv2
# Fails when a release binary is over its budget (SPEC 14.4 as amended by DECISIONS 3 for the
# signed update channel, #9): installer under 14 MB (about 8 MB before `tough` and its C crypto
# library, about 5 MB for TUF itself), and shim under 500 KB.
# The shim's 400 KB of the SPEC was measured on glibc (351 KB); the static musl build carries libc
# itself and is 443 KB at opt-level "s" or "z" alike, so the budget for the shipped build is 500 KB.
# usage: ci/size-check.sh <directory holding ciginstaller and cigi-shim>
set -euo pipefail
dir="${1:?usage: size-check.sh <release directory>}"
check() {
  local file="$dir/$1" limit="$2" size
  [[ -f "$file" ]] || { echo "::error::$file is missing"; exit 1; }
  size=$(wc -c < "$file" | tr -d ' ') # `stat -c` is GNU only; this runs on macOS too
  echo "$1: $size bytes (limit $limit)"
  if (( size >= limit )); then
    echo "::error::$1 is $size bytes, over its budget of $limit"
    exit 1
  fi
}
check ciginstaller 14000000
check cigi-shim 500000
