#!/bin/sh
# SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
# SPDX-License-Identifier: FSL-1.1-ALv2
# The family gate (Carton 1.0.0 rows 6 and 7): install a carton from its directory with the
# installer it carries, then prove each product and the products together. POSIX sh. The carton
# directory is what `cargo xtask carton-local` writes (a flat mirror with `carton-local.env`).
#
# usage: ci/carton-gate.sh <carton dir> [<cig-orchestrator tree>]
#   The cig-orchestrator tree gives the polyglot example (`examples/polyglot`), which cigo's
#   release archive does not carry. Without it that step fails and says why.
#   GATE_OUT=<dir> is where gate.json goes (default: the current directory).
#
# One line per step on stdout, `ok <step>` or `FAIL <step>: <why>`, the evidence in between; exit 1
# when any step failed. Everything runs under a scratch directory with its own HOME,
# CIGSCRIPT_HOME, CIGO_HOME and XDG directories; nothing outside it is touched. The steps run cig's
# burns and cigo's boxed tasks, so this runs on a test machine, not on a development machine.
set -u

dir="${1:?usage: carton-gate.sh <carton dir> [<cig-orchestrator tree>]}"
cigo_tree="${2:-}"
out="${GATE_OUT:-.}"
[ -f "$dir/carton-local.env" ] || { echo "FAIL setup: $dir/carton-local.env is missing"; exit 1; }
dir="$(cd "$dir" && pwd)"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
[ -z "$cigo_tree" ] || cigo_tree="$(cd "$cigo_tree" && pwd)"

# KEY=value lines, read as data (never sourced)
val() { sed -n "s/^$1=//p" "$dir/carton-local.env" | head -n 1; }
CARTON="$(val CARTON)"; CHANNEL="$(val CHANNEL)"; TARGET="$(val TARGET)"
CIG="$(val CIG)"; CIGO="$(val CIGO)"; CIGCOMPILE="$(val CIGCOMPILE)"; CIGAR="$(val CIGAR)"
INSTALLER="$(val CIGINSTALLER)"; INSTALLER_ASSET="$(val CIGINSTALLER_ASSET)"
manifest="$dir/manifest-$CHANNEL.toml"

# Not under /tmp: cig's box gives the processes it contains a /tmp of their own, so a project
# there would not be the one they see.
work="$(mktemp -d "$out/gate-work.XXXXXX")"
trap 'rm -rf "$work" 2> /dev/null || true' EXIT
STEP_TIMEOUT="${GATE_STEP_TIMEOUT:-900}"
T() { timeout "$STEP_TIMEOUT" "$@"; }

# A scratch home: HOME, the family's homes and every XDG directory inside it.
home_at() {
  export HOME="$1"
  export CIGSCRIPT_HOME="$1/.cigscript" CIGO_HOME="$1/.cig-orchestrator"
  export XDG_CONFIG_HOME="$1/.config" XDG_DATA_HOME="$1/.local/share"
  export XDG_CACHE_HOME="$1/.cache" XDG_STATE_HOME="$1/.local/state"
  mkdir -p "$1"
}
# The products' own test settings: no network lookups, no crash reports, the seal key in a file.
export NO_COLOR=1 CI="" CIG_DOCTOR=0 CIG_REPORTS=never CIG_SEAL_KEY=file
export CIGO_FLOOR_CACHE=off CIGO_OSV=off CIGO_REGISTRY_META=off CIGO_REHEARSE=never
export CIGCOMPILE_OFFLINE=1
# Every installer call, the shims' and cigo's included, reads this carton's directory.
export CIGI_MIRROR="file://$dir"
base_path="$PATH"
umask 022

results="$work/results"
why="$work/why"
: > "$results"
failed=0
# step <name> <function>: run it in a subshell under `set -e`, so its first failing check ends it;
# say ok or FAIL, and keep the result for gate.json.
step() {
  name="$1"; shift
  echo "== $name"
  : > "$why"
  ( set -e; "$@" )
  rc=$?
  if [ "$rc" = 0 ]; then
    echo "ok $name"
    printf '%s\tok\t\n' "$name" >> "$results"
  else
    failed=$((failed + 1))
    reason="$(tail -n 1 "$why" 2> /dev/null)"
    echo "FAIL $name: ${reason:-see the log above}"
    printf '%s\tFAIL\t%s\n' "$name" "$reason" >> "$results"
  fi
}
# say why a step fails; the last line written is the one reported
no() { echo "$*" | tee -a "$why" >&2; return 1; }
sha() { sha256sum "$1" | awk '{print $1}'; }
first2() { awk 'NR==1{print $1" "$2}'; }

echo "carton $CARTON ($CHANNEL) for $TARGET from $dir"
echo "cig $CIG, cigo $CIGO, cigcompile $CIGCOMPILE, cigar $CIGAR, ciginstaller $INSTALLER"
# shellcheck disable=SC1091 # the machine's own file, read for its name
( . /etc/os-release 2> /dev/null && echo "$PRETTY_NAME" ) || uname -a

# ---- the installer from the carton itself ----------------------------------------------------
inst="$work/installer"
ci="$inst/ciginstaller"
s_unpack() {
  mkdir -p "$inst"
  (cd "$dir" && sha256sum -c "$INSTALLER_ASSET.sha256") || no "the installer's sidecar does not match"
  tar -xzf "$dir/$INSTALLER_ASSET" -C "$inst" || no "cannot unpack $INSTALLER_ASSET"
  got="$("$ci" --version | first2)"
  [ "$got" = "ciginstaller $INSTALLER" ] || no "the carton's installer says '$got', not 'ciginstaller $INSTALLER'"
}
step installer-from-carton s_unpack

# ---- install -----------------------------------------------------------------------------------
home_at "$work/home"
PATH="$CIGSCRIPT_HOME/bin:$base_path"; export PATH
install_into() {
  # A directory is one source: allowed here as a user allows a private mirror (no pins are
  # compiled in, so nothing reaches `pinned`; see docs/CARTON-GATE.md).
  T "$ci" config set allow_single_source true || no "config set allow_single_source failed"
  T "$ci" install carton "$CARTON" --yes --from "$dir" || no "install carton $CARTON --from $dir failed"
  T "$ci" status | tee "$work/status.txt"
  grep -q "$CARTON" "$work/status.txt" || no "status does not name carton $CARTON"
}
s_install() { install_into; }
step install s_install

# ---- what is installed is what the manifest says ----------------------------------------------
tc() { echo "$CIGSCRIPT_HOME/toolchains/$1/$2"; }
# binary layout: the installed file is the asset, byte for byte
digest_binary() { # tool version asset bin
  a="$dir/$3"
  want="$(sha "$a")"
  (cd "$dir" && sha256sum -c "$3.sha256" > /dev/null) || no "$3: its sidecar does not match it"
  grep -q "\"$want\"" "$manifest" || no "$3: the manifest does not name $want"
  t="$(tc "$1" "$2")"
  [ -f "$t/bin/$4" ] || no "$1 $2: $t/bin/$4 is not installed"
  have="$(sha "$t/bin/$4")"
  [ "$have" = "$want" ] || no "$1 $2: installed $have, the manifest says $want"
  grep -q "$want" "$t/.cigi-receipt.json" || no "$1 $2: the receipt does not record $want"
  echo "$1 $2: $want"
}
# archive layout: the toolchain is the archive unpacked (strip as tools.toml says), file for file
digest_archive() { # tool version asset strip
  a="$dir/$3"
  want="$(sha "$a")"
  (cd "$dir" && sha256sum -c "$3.sha256" > /dev/null) || no "$3: its sidecar does not match it"
  grep -q "\"$want\"" "$manifest" || no "$3: the manifest does not name $want"
  t="$(tc "$1" "$2")"
  [ -d "$t" ] || no "$1 $2: $t is not installed"
  grep -q "$want" "$t/.cigi-receipt.json" || no "$1 $2: the receipt does not record $want"
  x="$work/unpacked-$1"
  rm -rf "$x"; mkdir -p "$x"
  if [ "$4" = 1 ]; then tar -xzf "$a" -C "$x" --strip-components=1; else tar -xzf "$a" -C "$x"; fi
  diff -r -x .cigi-receipt.json "$x" "$t" || no "$1 $2: the installed files differ from $3"
  echo "$1 $2: $want, $(find "$x" -type f | wc -l | tr -d ' ') files as unpacked"
}
s_digests() {
  digest_binary cig "$CIG" "$(val CIG_ASSET)" cig &&
    digest_archive cigo "$CIGO" "$(val CIGO_ASSET)" 1 &&
    digest_binary cigcompile "$CIGCOMPILE" "$(val CIGCOMPILE_ASSET)" cigcompile &&
    digest_archive cigar "$CIGAR" "$(val CIGAR_ASSET)" 0
}
step installed-is-the-manifest s_digests

# ---- every shim answers with the carton's version -------------------------------------------
lsp_init() { # the initialize, shutdown and exit messages, framed; prints the server's answer
  init='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"processId":null,"rootUri":null,"capabilities":{}}}'
  shut='{"jsonrpc":"2.0","id":2,"method":"shutdown"}'
  bye='{"jsonrpc":"2.0","method":"exit"}'
  for m in "$init" "$shut" "$bye"; do
    printf 'Content-Length: %s\r\n\r\n%s' "$(printf '%s' "$m" | wc -c | tr -d ' ')" "$m"
  done | T cigar-lsp
}
s_shims() {
  for s in cig cigo cigcompile cigar cigar-lsp; do
    [ "$(command -v "$s")" = "$CIGSCRIPT_HOME/bin/$s" ] || no "$s on PATH is $(command -v "$s"), not the carton's shim"
  done
  got="$(T cig --version | first2)"; echo "cig: $got"
  [ "$got" = "cig $CIG" ] || no "cig --version says '$got', not 'cig $CIG'"
  got="$(T cigo --version | head -n 1)"; echo "cigo: $got"
  [ "$got" = "cig-orchestrator $CIGO (cig $CIG)" ] || no "cigo --version says '$got', not 'cig-orchestrator $CIGO (cig $CIG)'"
  got="$(T cigcompile --version | head -n 1)"; echo "cigcompile: $got"
  [ "$got" = "cigcompile $CIGCOMPILE (makes programs for cig $CIG)" ] ||
    no "cigcompile --version says '$got', not 'cigcompile $CIGCOMPILE (makes programs for cig $CIG)'"
  got="$(T cigar --version | first2)"; echo "cigar: $got"
  [ "$got" = "cigar $CIGAR" ] || no "cigar --version says '$got', not 'cigar $CIGAR'"
  # cigar-lsp has no --version: its version is in initialize's serverInfo
  got="$(lsp_init)" || no "cigar-lsp did not exit 0 after shutdown and exit"
  echo "$got" | grep -q '"name":"cigar-lsp"' || no "cigar-lsp's initialize answer has no serverInfo: $got"
  echo "$got" | grep -q "\"version\":\"$CIGAR\"" || no "cigar-lsp's initialize answer names no version $CIGAR: $got"
}
step every-shim-answers s_shims

# ---- doctor and check ----------------------------------------------------------------------
# A fresh home has no cigo guard installed, and cigo's doctor says so as two actions (its
# package-manager shims, and the startup file that puts them first). Those two are named here;
# any other finding fails the step.
DOCTOR_EXPECTED='cigo: guard-shims:|cigo: guard-path:'
s_doctor() {
  rc=0; T "$ci" doctor > "$work/doctor.txt" 2>&1 || rc=$?
  cat "$work/doctor.txt"
  [ "$rc" = 0 ] && return 0
  [ "$rc" = 2 ] || no "ciginstaller doctor exited $rc"
  other="$(grep '^✗ ' "$work/doctor.txt" | grep -v 'thing(s) need action' | grep -Ev "^✗ ($DOCTOR_EXPECTED)" || true)"
  [ -z "$other" ] || no "doctor found: $other"
  echo "doctor: only the named findings of a home with no cigo guard ($DOCTOR_EXPECTED)"
}
step doctor s_doctor
s_check() { T "$ci" check || no "ciginstaller check exited $?"; }
step check s_check

# ---- cig: a failing burn is rolled back; a good run is listed and taken back ------------------
cigp="$work/proj-cig"
mkdir -p "$cigp"
newest_run() { T cig runs | awk 'NR==2{print $1" "$2}'; }
s_cig_rollback() {
  cat > "$cigp/bad.cig" << 'EOF'
burn {
  fs.write_text("a.txt", "one")
  fs.write_text("b.txt", "two")
}
cough "abort"
EOF
  rc=0; (cd "$cigp" && T cig run bad.cig) || rc=$?
  [ "$rc" = 1 ] || no "cig run bad.cig exited $rc, not 1 (a script error)"
  if [ -e "$cigp/a.txt" ] || [ -e "$cigp/b.txt" ]; then no "the failed burn left a.txt or b.txt behind"; fi
  got="$(cd "$cigp" && newest_run)"; echo "newest run: $got"
  [ "${got#* }" = rolled_back ] || no "the failed run is '$got', not rolled_back"
}
step cig-failed-burn-rolls-back s_cig_rollback
s_cig_unburn() {
  printf 'burn {\n  fs.write_text("c.txt", "kept")\n}\n' > "$cigp/good.cig"
  (cd "$cigp" && T cig run good.cig) || no "cig run good.cig failed"
  [ -f "$cigp/c.txt" ] || no "the good burn wrote no c.txt"
  got="$(cd "$cigp" && newest_run)"; echo "newest run: $got"
  id="${got%% *}"
  [ "${got#* }" = ok ] || no "cig runs lists '$got', not an ok run"
  (cd "$cigp" && T cig unburn "$id") || no "cig unburn $id failed"
  [ ! -e "$cigp/c.txt" ] || no "cig unburn $id left c.txt"
  got="$(cd "$cigp" && newest_run)"; echo "after unburn: $got"
  [ "$got" = "$id unburned" ] || no "after unburn the run is '$got', not '$id unburned'"
}
step cig-runs-and-unburn s_cig_unburn

# ---- cigo: run ci passes; a failing task rolls back what the run wrote -------------------------
cigop="$work/proj-cigo"
mkdir -p "$cigop"
cat > "$cigop/cig-tasks.json" << 'EOF'
{"project": "gate",
 "tasks": {
  "build": {"shell": "echo built > out.txt", "produces": ["out.txt"]},
  "test": {"shell": "test -f out.txt", "needs": ["build"]},
  "bad": {"shell": "echo half > half.txt && exit 3"},
  "homes": {"shell": "echo \"HOME=$HOME\"; echo \"CIGSCRIPT_HOME=$CIGSCRIPT_HOME\"; echo \"XDG_CONFIG_HOME=$XDG_CONFIG_HOME\"; echo \"CIGI_MIRROR=$CIGI_MIRROR\"; ciginstaller config get allow_single_source; echo \"which=$(ciginstaller which cig --path)\""},
  "shims": {"shell": "cig --version; cigo --version; cigcompile --version; cigar --version; ciginstaller --version"},
  "install": {"shell": "ciginstaller install carton --yes"}
 }}
EOF
s_cigo_ci() {
  (cd "$cigop" && T cigo run ci) || no "cigo run ci failed"
  [ -f "$cigop/out.txt" ] || no "cigo run ci wrote no out.txt"
}
step cigo-run-ci s_cigo_ci
s_cigo_rollback() {
  rc=0; (cd "$cigop" && T cigo run bad) || rc=$?
  [ "$rc" != 0 ] || no "cigo run bad passed"
  [ ! -e "$cigop/half.txt" ] || no "the failed run left half.txt behind"
}
step cigo-failed-task-rolls-back s_cigo_rollback
# Inside a task (cig's box) CIGSCRIPT_HOME is the run's inbox, cig's state root there. The family
# home is where the tools live (decision 0005): ciginstaller run inside the task answers for this
# home, its settings and its toolchains.
s_cigo_homes() {
  got="$(cd "$cigop" && T cigo run homes 2>&1)" || { echo "$got"; no "cigo run homes failed"; }
  echo "$got"
  for v in "HOME=$HOME" "XDG_CONFIG_HOME=$XDG_CONFIG_HOME" "CIGI_MIRROR=$CIGI_MIRROR"; do
    echo "$got" | grep -qF "$v" || no "a cigo task does not see $v"
  done
  echo "$got" | grep -q 'allow_single_source = true' || no "ciginstaller in a cigo task does not see allow_single_source = true"
  echo "$got" | grep -qF "which=$(tc cig "$CIG")/bin/cig" || no "ciginstaller in a cigo task does not name this home's cig $CIG"
}
step cigo-task-sees-the-family-home s_cigo_homes
# Every shim run inside a boxed task starts the carton's build.
s_task_shims() {
  got="$(cd "$cigop" && T cigo run shims 2>&1)" || { echo "$got"; no "cigo run shims failed"; }
  echo "$got"
  for v in "cig $CIG" "cig-orchestrator $CIGO (cig $CIG)" "cigcompile $CIGCOMPILE (makes programs for cig $CIG)" "cigar $CIGAR" "ciginstaller $INSTALLER"; do
    echo "$got" | grep -qF "$v" || no "inside a cigo task no shim says '$v'"
  done
}
step shims-answer-inside-a-task s_task_shims
# An install from inside a task is refused with E1905, and the home is as it was.
family_files() { (cd "$CIGSCRIPT_HOME" && find bin toolchains installer humidor -type f -exec sha256sum {} + 2> /dev/null | sort); }
s_task_install() {
  family_files > "$work/family-before.txt"
  rc=0; got="$(cd "$cigop" && T cigo run install 2>&1)" || rc=$?
  echo "$got"
  [ "$rc" != 0 ] || no "an install inside a cigo task passed"
  echo "$got" | grep -q E1905 || no "an install inside a cigo task was not refused with E1905"
  family_files > "$work/family-after.txt"
  diff "$work/family-before.txt" "$work/family-after.txt" || no "the refused install changed the home"
}
step install-inside-a-task-is-refused s_task_install

# ---- CigCompile: build, run and verify a hello and a burn, with the carton's cig ----------------
ccp="$work/proj-cc"
mkdir -p "$ccp"
rt="$(tc cig "$CIG")/bin/cig"
# shellcheck disable=SC2016 # ${name} is CigScript's, not the shell's
printf 'stick name = args.get(0, "world")\nexhale "hello, ${name}"\n' > "$ccp/hello.cig"
printf 'burn {\n  fs.write_text("w.txt", "x")\n}\nexhale "burned"\n' > "$ccp/burn.cig"
s_cc_build() {
  for p in hello burn; do
    (cd "$ccp" && T cigcompile build "$p.cig" -o "$p" --runtime "$rt") || no "cigcompile build $p.cig failed"
  done
  got="$(cd "$ccp" && T ./hello gate)"; echo "$got"
  [ "$got" = "hello, gate" ] || no "./hello said '$got'"
  (cd "$ccp" && T ./burn) || no "./burn failed"
  [ -f "$ccp/w.txt" ] || no "./burn wrote no w.txt"
  for p in hello burn; do
    (cd "$ccp" && T cigcompile verify "$p" --runtime "$rt" --script "$p.cig") || no "cigcompile verify $p failed"
  done
}
step cigcompile-build-run-verify s_cc_build
# Without --runtime (and no network), cigcompile looks beside itself, on PATH and in cig's
# humidor for the cig it pins; the carton put that cig on the machine.
s_cc_finds() {
  (cd "$ccp" && T cigcompile build hello.cig -o hello2) || no "cigcompile does not find the carton's cig $CIG by itself"
}
step cigcompile-finds-the-cartons-cig s_cc_finds

# ---- CigarScript: check at each strength, run a program, the language server ------------------
cgp="$work/proj-cigar"
mkdir -p "$cgp"
printf 'strength mild\n\nexhale "hello from CigarScript"\n' > "$cgp/mild.cigar"
printf 'strength medium\nrecord Move { from: Path, to: Path }\n' > "$cgp/medium.cigar"
printf 'strength full\n\nstick a: u8 = 200\nstick b: u8 = 55\nstick c: u8 = a + b\nexhale a, b, c\n' > "$cgp/full.cigar"
s_cigar_check() {
  for f in mild medium full; do
    (cd "$cgp" && T cigar check "$f.cigar") || no "cigar check $f.cigar failed"
  done
}
step cigar-check-three-strengths s_cigar_check
s_cigar_run() {
  got="$(cd "$cgp" && T cigar run full.cigar)"; echo "$got"
  [ "$got" = "200 55 255" ] || no "cigar run full.cigar said '$got', not '200 55 255'"
  got="$(cd "$cgp" && T cigar run mild.cigar)"; echo "$got"
  [ "$got" = "hello from CigarScript" ] || no "cigar run mild.cigar said '$got'"
}
step cigar-run s_cigar_run
s_lsp() {
  got="$(lsp_init)" || no "cigar-lsp did not exit 0 after shutdown and exit"
  echo "$got" | grep -q '"capabilities"' || no "cigar-lsp's initialize answer has no capabilities: $got"
}
step cigar-lsp-initialize s_lsp

# ---- the products together: cigo's polyglot example with the real cigcompile and cigar ---------
s_polyglot() {
  if [ -z "$cigo_tree" ] || [ ! -d "$cigo_tree/examples/polyglot" ]; then
    no "no cig-orchestrator tree with examples/polyglot was given"
  fi
  for t in python3 node bash; do command -v "$t" > /dev/null || no "the polyglot example needs $t"; done
  pg="$work/polyglot"
  cp -R "$cigo_tree/examples/polyglot" "$pg"
  (cd "$pg" && T cigo run ci) > "$work/poly-ci.txt" 2>&1 || { cat "$work/poly-ci.txt"; no "cigo run ci in the polyglot example failed"; }
  cat "$work/poly-ci.txt"
  for s in '] cigar:lint:' '] cigar:check:' '] cigar:build:' '] cigar:test:'; do
    grep -qF "$s" "$work/poly-ci.txt" || no "the polyglot ci ran no '$s'"
  done
  [ -f "$pg/cigar/bin/release-note" ] || no "the polyglot ci built no cigar/bin/release-note"
  (cd "$pg" && T cigo run release) > "$work/poly-rel.txt" 2>&1 || { cat "$work/poly-rel.txt"; no "cigo run release in the polyglot example failed"; }
  cat "$work/poly-rel.txt"
  grep -qF 'live: public/2.3.0/index.html' "$work/poly-rel.txt" || no "the polyglot release did not go live"
}
step products-together-polyglot s_polyglot

# ---- the installer's own lifecycle, in a second fresh home -------------------------------------
home_at "$work/home2"
PATH="$CIGSCRIPT_HOME/bin:$base_path"; export PATH
s_install2() { install_into; }
step lifecycle-install s_install2
s_pin() {
  p="$work/pin-ok"; q="$work/pin-other"
  mkdir -p "$p" "$q"
  (cd "$p" && T "$ci" pin cig "$CIG") || no "ciginstaller pin cig $CIG failed"
  got="$(cd "$p" && T cig --version | first2)"; echo "pinned to $CIG: $got"
  [ "$got" = "cig $CIG" ] || no "with cig pinned to $CIG, cig says '$got'"
  # a pin the carton does not hold must not run the carton's cig
  (cd "$q" && T "$ci" pin cig 1.0.0) || printf '1.0.0\n' > "$q/.cig-version"
  ls -A "$q"; cat "$q/.cig-version" 2> /dev/null || true
  rc=0; got="$(cd "$q" && T cig --version 2>&1)" || rc=$?
  echo "pinned to 1.0.0 (exit $rc): $got"
  [ "$rc" != 0 ] || no "with cig pinned to 1.0.0, cig ran and said '$got'"
  T "$ci" which cig --cwd "$p" --why || no "ciginstaller which cig --cwd failed"
}
step pin-is-honored s_pin
s_repair() {
  rm -f "$CIGSCRIPT_HOME/bin/cigo"
  f="$(tc cig "$CIG")/bin/cig"
  chmod u+w "$f" || no "could not make $f writable"
  printf 'damage' >> "$f" || no "could not damage $f"
  T "$ci" --yes repair || no "ciginstaller repair failed"
  [ -x "$CIGSCRIPT_HOME/bin/cigo" ] || no "repair did not put the cigo shim back"
  [ "$(sha "$f")" = "$(sha "$dir/$(val CIG_ASSET)")" ] || no "repair did not restore $f"
  got="$(T cigo --version | head -n 1)"; echo "after repair: $got"
  [ "$got" = "cig-orchestrator $CIGO (cig $CIG)" ] || no "after repair cigo says '$got'"
}
step repair-shim-and-toolchain s_repair
# `uninstall --all` keeps the user's data and says which: "stays in <CIGSCRIPT_HOME>: a/, b, ...".
# What it names stays; anything else left in the home fails the step, and every path is printed.
s_uninstall() {
  # removing everything cannot be undone, so it takes --yes-irreversible (E1871 without it)
  T "$ci" --yes --yes-irreversible uninstall --all > "$work/uninstall.txt" 2>&1 || { cat "$work/uninstall.txt"; no "ciginstaller uninstall --all failed"; }
  cat "$work/uninstall.txt"
  [ -d "$HOME" ] || { echo "the home itself is gone"; return 0; }
  left="$(cd "$HOME" && find . -mindepth 1 | sort)"
  [ -n "$left" ] || return 0
  echo "left in the scratch home:"; echo "$left"
  # each named entry, its note in parentheses dropped: a name inside the home (`runs/`), or a
  # whole path beside it (cig's key folder); as paths relative to $HOME
  stays="$(sed -n 's/.*stays in [^:]*: \([^;]*\);.*/\1/p' "$work/uninstall.txt" | head -n 1 | tr ',' '\n' |
    sed 's/ *(.*$//; s/^ *//; s/ *$//; s|/$||' | while IFS= read -r k; do
      case "$k" in
        "$HOME"/*) echo "./${k#"$HOME"/}" ;;
        /*) echo "$k" ;;
        ?*) echo "./.cigscript/$k" ;;
      esac
    done)"
  echo "the installer says these stay:"; echo "$stays"
  unexplained="$(echo "$left" | while IFS= read -r p; do
    [ "$p" = ./.cigscript ] && continue
    ok=0
    for k in $stays; do
      case "$p" in "$k" | "$k"/*) ok=1 ;; esac
    done
    [ "$ok" = 1 ] || echo "$p"
  done)"
  [ -z "$unexplained" ] || no "uninstall --all left paths it does not say stay: $(echo "$unexplained" | tr '\n' ' ')"
}
step uninstall-all-leaves-nothing s_uninstall

# ---- gate.json --------------------------------------------------------------------------------
total="$(wc -l < "$results" | tr -d ' ')"
{
  printf '{\n  "carton": "%s",\n  "channel": "%s",\n  "target": "%s",\n' "$CARTON" "$CHANNEL" "$TARGET"
  printf '  "products": {"cig": "%s", "cigo": "%s", "cigcompile": "%s", "cigar": "%s", "ciginstaller": "%s"},\n' \
    "$CIG" "$CIGO" "$CIGCOMPILE" "$CIGAR" "$INSTALLER"
  printf '  "manifest_sha256": "%s",\n' "$(sha "$manifest")"
  printf '  "finished": "%s",\n  "passed": %s,\n  "failed": %s,\n  "steps": [\n' \
    "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$((total - failed))" "$failed"
  n=0
  while IFS="$(printf '\t')" read -r s r w; do
    n=$((n + 1))
    w="$(printf '%s' "$w" | sed 's/\\/\\\\/g; s/"/\\"/g')"
    sep=","; [ "$n" = "$total" ] && sep=""
    printf '    {"step": "%s", "result": "%s", "why": "%s"}%s\n' "$s" "$r" "$w" "$sep"
  done < "$results"
  printf '  ]\n}\n'
} > "$out/gate.json"
echo "gate: $((total - failed)) ok, $failed failed; $out/gate.json"
[ "$failed" = 0 ]
