#!/bin/sh
# SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
# SPDX-License-Identifier: FSL-1.1-ALv2
# Build a local carton from five source trees, then run the family gate on it. POSIX sh. Run it
# from this repository's root on a machine that may make boxes (the gate runs cigo's runs and cig's
# burns): a test machine, never a development machine.
#
# usage: ci/carton-e2e.sh --work <dir> --carton <v> --cig <tree> --cigo <tree>
#          --cigcompile <tree> --cigar <tree> --installer <tree> [--build-only]
#
# <work> keeps the build output between runs (<work>/build/<tool>/target), so a second run is warm.
# Each run replaces <work>/carton (the carton directory) and <work>/gate (gate.log, gate.json).
# The cig-orchestrator tree also gives the gate cigo's polyglot example.
set -eu

work="" carton="" build_only=0 cigo_tree=""
trees=""
while [ $# -gt 0 ]; do
  case "$1" in
    --work) work="$2"; shift 2 ;;
    --carton) carton="$2"; shift 2 ;;
    --cigo) cigo_tree="$2"; trees="$trees $1 $2"; shift 2 ;;
    --cig|--cigcompile|--cigar|--installer) trees="$trees $1 $2"; shift 2 ;;
    --build-only) build_only=1; shift ;;
    *) echo "carton-e2e: unknown argument $1" >&2; exit 2 ;;
  esac
done
[ -n "$work" ] || { echo "carton-e2e: --work is required" >&2; exit 2; }
[ -n "$carton" ] || { echo "carton-e2e: --carton is required" >&2; exit 2; }
[ -f xtask/Cargo.toml ] || { echo "carton-e2e: run it from cig-installer's root" >&2; exit 2; }

mkdir -p "$work"
work="$(cd "$work" && pwd)"
rm -rf "$work/carton" "$work/gate"
echo "== cargo xtask carton-local (build output in $work/build)"
# shellcheck disable=SC2086 # $trees is pairs of flag and path, split on purpose
cargo xtask carton-local --carton "$carton" --out "$work/carton" --work "$work/build" $trees
ls -l "$work/carton"
if [ "$build_only" = 1 ]; then
  echo "carton $carton built in $work/carton"
  exit 0
fi
mkdir -p "$work/gate"
echo "== ci/carton-gate.sh $work/carton (log and gate.json in $work/gate)"
rc=0
GATE_OUT="$work/gate" sh ci/carton-gate.sh "$work/carton" "$cigo_tree" > "$work/gate/gate.log" 2>&1 || rc=$?
grep -E '^(ok|FAIL) ' "$work/gate/gate.log" || true
echo "gate exit $rc; the whole log is $work/gate/gate.log, the summary $work/gate/gate.json"
exit "$rc"
