#!/bin/sh
# SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
# SPDX-License-Identifier: FSL-1.1-ALv2
# The performance budgets of #20, measured on a RELEASE build (the limits in bench/bench.toml apply
# only there), by the `bench` workflow on Linux and macOS. It installs the fixture carton into a
# scratch home so `check` has cached metadata to read, then runs `ciginstaller bench --strict` and
# fails loudly if any row is over, if any row this job must measure was not measured, or if the
# binary is not a release build. The two rows `bench` cannot measure (the update check and disk for
# five cigo versions against real stores) are measured by the release-mode budget tests, which the
# same job runs. Everything happens under a scratch HOME and CIGSCRIPT_HOME.
# usage: ci/bench-release.sh <release-dir>   (holds ciginstaller, cigi-shim and xtask)
set -eu

dir="${1:?usage: bench-release.sh <release-dir>}"
for f in ciginstaller cigi-shim xtask; do
  [ -x "$dir/$f" ] || { echo "::error::$dir/$f is missing or not executable"; exit 1; }
done
command -v jq > /dev/null || { echo "::error::jq is needed to read the bench report"; exit 1; }

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/home"
export HOME="$work/home"
export CIGSCRIPT_HOME="$work/home/.cigscript"
export NO_COLOR=1
# the fixture mirror is one source, which the installer refuses unless the user allows it (as the
# tests and the fresh-install job do); `bench --from` installs into homes of its own
export CIGI_ALLOW_SINGLE_SOURCE=true
ci="$dir/ciginstaller"

echo "== the fixture mirror and one installed carton (the cached metadata \`check\` reads)"
"$dir/xtask" fixture-mirror "$work/mirror"
"$ci" install carton --yes --no-path --from "$work/mirror"

echo "== ciginstaller bench --strict (release build)"
status=0
"$ci" --plain bench --strict --runs 100 --from "$work/mirror" || status=$?

# the same run as data, for the checks below (and the table when the strict run failed)
"$ci" --json bench --runs 100 --from "$work/mirror" > "$work/bench.json"
jq -r '.data.budgets[] | "\(.status)\t\(.id)\t\(.median // "-") (limit \(.limit) \(.unit))\t\(.note)"' "$work/bench.json"

if [ "$status" -ne 0 ]; then
  echo "::error::ciginstaller bench --strict failed (exit $status): a budget is over, or bench could not run"
  exit 1
fi

build="$(jq -r '.data.build' "$work/bench.json")"
[ "$build" = "release" ] || { echo "::error::this is a $build build; the budgets are judged on a release build"; exit 1; }

# a row that was not measured is not a pass: name every row this job must measure
missing=""
for id in shim_dispatch check switch installer_bytes shim_bytes first_line_status first_line_list install_pack_dev install_carton; do
  s="$(jq -r --arg id "$id" '.data.budgets[] | select(.id == $id) | .status' "$work/bench.json")"
  [ "$s" = "ok" ] || missing="$missing $id($s)"
done
if [ -n "$missing" ]; then
  echo "::error::rows not measured as ok:$missing"
  exit 1
fi
echo "release bench ok: every row this job measures is within its budget"
