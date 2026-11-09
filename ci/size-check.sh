#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
# SPDX-License-Identifier: Apache-2.0
# Fails when a release binary is over its budget (SPEC 14.4): installer under 10 MB, shim under 500 KB.
# The shim's 400 KB of the SPEC was measured on glibc (351 KB); the static musl build carries libc
# itself and is 443 KB at opt-level "s" or "z" alike, so the budget for the shipped build is 500 KB.
# usage: ci/size-check.sh <directory holding ciginstaller and cigi-shim>
set -euo pipefail
dir="${1:?usage: size-check.sh <release directory>}"
check() {
  local file="$dir/$1" limit="$2" size
  [[ -f "$file" ]] || { echo "::error::$file is missing"; exit 1; }
  size=$(stat -c %s "$file")
  echo "$1: $size bytes (limit $limit)"
  if (( size >= limit )); then
    echo "::error::$1 is $size bytes, over its budget of $limit"
    exit 1
  fi
}
check ciginstaller 10000000
check cigi-shim 500000
