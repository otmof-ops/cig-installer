#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
# SPDX-License-Identifier: FSL-1.1-ALv2
# Tests the bootstrap scripts against a local fixture: a fake tarball served by file:// and by a
# throwaway HTTP server on 127.0.0.1. Fills the templates the way the release workflow does, then
# runs the filled install.sh with scratch homes only. Nothing here touches a real home, profile or
# release, and nothing leaves the machine.
# usage: ci/test-bootstrap.sh
set -uo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
work="$(mktemp -d)"
server_pid=""
cleanup() {
  [[ -n "$server_pid" ]] && kill "$server_pid" 2>/dev/null
  rm -rf "$work"
}
trap cleanup EXIT

fails=0
pass() { echo "ok   $*"; }
fail() { echo "FAIL $*"; fails=$((fails + 1)); }
check() { # check <description> <command...>
  local what="$1"; shift
  if "$@"; then pass "$what"; else fail "$what"; fi
}

ver=9.9.9
tag=v$ver
repo=test-owner/test-installer

# ---- fixture: a fake ciginstaller in a tarball per platform, with sidecars
fx="$work/fixture"
mkdir -p "$fx/stage" "$work/bin" "$work/tmp"
cat > "$fx/stage/ciginstaller" <<'FAKE'
#!/bin/sh
# a stand-in: a failing install leaves nothing (the real one is a burn); a good one records how it ran
if [ "${FAKE_EXIT:-0}" -ne 0 ]; then exit "$FAKE_EXIT"; fi
mkdir -p "$CIGSCRIPT_HOME"
printf '%s\n' "$@" > "$CIGSCRIPT_HOME/args.txt"
echo "ran from $(dirname "$0")" > "$CIGSCRIPT_HOME/ran-from.txt"
FAKE
printf '#!/bin/sh\nexit 0\n' > "$fx/stage/cigi-shim"
echo licence > "$fx/stage/LICENSE"; echo notice > "$fx/stage/NOTICE"; echo third > "$fx/stage/THIRD-PARTY-NOTICES"
chmod +x "$fx/stage/ciginstaller" "$fx/stage/cigi-shim"
for p in linux-x86_64 linux-aarch64; do
  name="ciginstaller-v$ver-$p.tar.gz"
  tar --sort=name --owner=0 --group=0 --numeric-owner -C "$fx/stage" -cf - . | gzip -n > "$fx/$name"
  (cd "$fx" && sha256sum "$name" > "$name.sha256")
done
bash "$here/ci/fill-bootstrap.sh" "$tag" "$repo" "$fx" "$work/out" > "$work/fill.log" 2>&1 \
  || { cat "$work/fill.log"; echo "FAIL fill-bootstrap.sh"; exit 1; }
pass "the templates fill"
script="$work/out/install.sh"

# ---- stubs: uname (to pretend to be other machines) and gh
cat > "$work/bin/uname" <<'STUB'
#!/bin/sh
case "$1" in
  -s) echo "${FAKE_S:-Linux}" ;;
  -m) echo "${FAKE_M:-x86_64}" ;;
  *) echo "${FAKE_S:-Linux}" ;;
esac
STUB
chmod +x "$work/bin/uname"
mkgh() { # mkgh <exit code>: a signed-in gh that records its arguments; `attestation verify` exits with that code
  # shellcheck disable=SC2016 # the stub's own $1 $2 and $@, expanded when it runs
  printf '#!/bin/sh\necho "$@" >> "%s/gh-calls.txt"\n[ "$1 $2" = "auth status" ] && exit 0\nexit %s\n' "$work" "$1" > "$work/ghbin/gh"
  chmod +x "$work/ghbin/gh"
}
mkdir -p "$work/ghbin"
# the default gh on PATH for every other case: installed, not signed in (so the real gh never runs)
# shellcheck disable=SC2016 # the stub's own $1 $2, expanded when it runs
printf '#!/bin/sh\n[ "$1 $2" = "auth status" ] && exit 1\necho "gh: not signed in" >&2\nexit 4\n' > "$work/bin/gh"
chmod +x "$work/bin/gh"

# run <home> [args...]: the filled script, scratch TMPDIR, the fixture over file://
run() {
  local home="$1"; shift
  PATH="$work/bin:$PATH" TMPDIR="$work/tmp" CIGSCRIPT_HOME="$home" \
    CIGI_BOOTSTRAP_BASE_URL="file://$fx" sh "$script" "$@" > "$work/stdout" 2> "$work/stderr"
}
scratch_empty() { [[ -z "$(ls -A "$work/tmp")" ]]; }
said() { grep -qF -- "$1" "$work/stderr"; }

# 1. a good digest installs, passes arguments through, and cleans up
home="$work/home1"
run "$home" --yes carton --preset strict; rc=$?
check "good digest: exit 0" test "$rc" -eq 0
check "good digest: the installer ran with every argument" test "$(tr '\n' ' ' < "$home/args.txt")" = "install --yes carton --preset strict "
check "good digest: it ran from under the home" grep -q "^ran from $home/installer/bootstrap$" "$home/ran-from.txt"
check "good digest: the bootstrap directory is gone afterwards" test ! -e "$home/installer/bootstrap"
check "good digest: no scratch files left" scratch_empty
check "good digest: it says the digest was checked" said "sha256 ok"

# 2. a wrong digest refuses and leaves nothing behind
cp "$fx/ciginstaller-v$ver-linux-x86_64.tar.gz" "$work/good.tgz"
printf 'tampered' >> "$fx/ciginstaller-v$ver-linux-x86_64.tar.gz"
home="$work/home2"
run "$home" --yes carton; rc=$?
check "wrong digest: refused" test "$rc" -ne 0
check "wrong digest: it says why" said "does not match the digest"
check "wrong digest: the home was never created" test ! -e "$home"
check "wrong digest: no scratch files left" scratch_empty
mkdir -p "$work/home2b/keep"; echo mine > "$work/home2b/keep/file"
run "$work/home2b" --yes carton; rc=$?
check "wrong digest into an existing home: refused, home untouched" test "$rc" -ne 0 -a "$(ls -A "$work/home2b")" = keep -a ! -e "$work/home2b/installer"
cp "$work/good.tgz" "$fx/ciginstaller-v$ver-linux-x86_64.tar.gz"

# 3. platforms with no build refuse clearly
for combo in "Linux riscv64" "Darwin arm64" "FreeBSD amd64"; do
  read -r s m <<< "$combo"
  home="$work/home3"
  FAKE_S="$s" FAKE_M="$m" run "$home" --yes carton; rc=$?
  check "$s $m: refused" test "$rc" -ne 0
  check "$s $m: it names the problem" said "no ciginstaller build for"
  check "$s $m: nothing was created" test ! -e "$home"
done

# 4. aarch64 picks its own tarball
home="$work/home4"
FAKE_M=aarch64 run "$home" --yes carton; rc=$?
check "aarch64: installs" test "$rc" -eq 0 -a -f "$home/args.txt"
check "aarch64: it names the platform" said "linux-aarch64"

# 5. a failing install: its exit code is passed on and the home this run made is removed
home="$work/home5"
FAKE_EXIT=7 run "$home" --yes carton; rc=$?
check "failing install: exit code 7 is passed on" test "$rc" -eq 7
check "failing install: it says the machine is as it was" said "the machine is as it was"
check "failing install: the home it created is removed" test ! -e "$home"

# 6. gh: a failing attestation refuses, a passing one installs, skip bypasses it and says so
mkgh 1
home="$work/home6a"
PATH="$work/ghbin:$work/bin:$PATH" TMPDIR="$work/tmp" CIGSCRIPT_HOME="$home" CIGI_BOOTSTRAP_BASE_URL="file://$fx" \
  sh "$script" --yes carton > "$work/stdout" 2> "$work/stderr"; rc=$?
check "failing attestation: refused" test "$rc" -ne 0
check "failing attestation: it says so" said "gh attestation verify failed"
check "failing attestation: gh was asked about this repository" grep -q -- "attestation verify .* -R $repo" "$work/gh-calls.txt"
check "failing attestation: nothing left" test ! -e "$home"
mkgh 0
home="$work/home6b"
PATH="$work/ghbin:$work/bin:$PATH" TMPDIR="$work/tmp" CIGSCRIPT_HOME="$home" CIGI_BOOTSTRAP_BASE_URL="file://$fx" \
  sh "$script" --yes carton > "$work/stdout" 2> "$work/stderr"; rc=$?
check "passing attestation: installs" test "$rc" -eq 0 -a -f "$home/args.txt"
check "passing attestation: it says so" said "attestation ok"
mkgh 1
home="$work/home6c"
PATH="$work/bin:$PATH" TMPDIR="$work/tmp" CIGSCRIPT_HOME="$home" CIGI_BOOTSTRAP_BASE_URL="file://$fx" \
  sh "$script" --yes carton > "$work/stdout" 2> "$work/stderr"; rc=$?
check "gh not signed in: installs on the digest alone" test "$rc" -eq 0 -a -f "$home/args.txt"
check "gh not signed in: it says provenance was not checked" said "gh is not signed in"

# 7. the override accepts only https, loopback http or file
home="$work/home7"
CIGI_BOOTSTRAP_BASE_URL="http://example.com/x" PATH="$work/bin:$PATH" TMPDIR="$work/tmp" CIGSCRIPT_HOME="$home" \
  sh "$script" --yes carton > "$work/stdout" 2> "$work/stderr"; rc=$?
check "plain http to a remote host: refused" test "$rc" -ne 0 -a ! -e "$home"
check "plain http: it says why" said "must be https"

# 8. a relative home, an unfilled template
CIGSCRIPT_HOME=relative/home PATH="$work/bin:$PATH" TMPDIR="$work/tmp" CIGI_BOOTSTRAP_BASE_URL="file://$fx" \
  sh "$script" --yes carton > "$work/stdout" 2> "$work/stderr"; rc=$?
check "relative CIGSCRIPT_HOME: refused" test "$rc" -ne 0 -a ! -e relative
TMPDIR="$work/tmp" sh "$here/packaging/bootstrap/install.sh.in" --yes carton > "$work/stdout" 2> "$work/stderr"; rc=$?
check "the unfilled template refuses" test "$rc" -ne 0
check "the template says what it is" said "this is the template"
check "--help prints and exits 0" sh -c "sh '$script' --help 2> /dev/null"

# 9. a tarball with a path that climbs out, or a symlink, is refused (the digest is right; the contents are not)
if command -v python3 > /dev/null; then
  bad="$work/bad"; mkdir -p "$bad"
  for kind in climb link; do
    python3 -I - "$bad" "$ver" "$kind" <<'PY'
import io, sys, tarfile
d, ver, kind = sys.argv[1:4]
for p in ("linux-x86_64", "linux-aarch64"):
    with tarfile.open(f"{d}/ciginstaller-v{ver}-{p}.tar.gz", "w:gz") as t:
        for n in ("ciginstaller", "cigi-shim"):
            i = tarfile.TarInfo(n); i.size = 3; i.mode = 0o755
            t.addfile(i, io.BytesIO(b"#!\n"))
        if kind == "climb":
            i = tarfile.TarInfo("../evil"); i.size = 1
            t.addfile(i, io.BytesIO(b"x"))
        else:
            i = tarfile.TarInfo("sneaky"); i.type = tarfile.SYMTYPE; i.linkname = "/etc"
            t.addfile(i)
PY
    (cd "$bad" && for f in "ciginstaller-v$ver"-*.tar.gz; do sha256sum "$f" > "$f.sha256"; done)
    bash "$here/ci/fill-bootstrap.sh" "$tag" "$repo" "$bad" "$work/out-bad" > /dev/null 2>&1
    home="$work/home9-$kind"
    PATH="$work/bin:$PATH" TMPDIR="$work/tmp" CIGSCRIPT_HOME="$home" CIGI_BOOTSTRAP_BASE_URL="file://$bad" \
      sh "$work/out-bad/install.sh" --yes carton > "$work/stdout" 2> "$work/stderr"; rc=$?
    check "unsafe tarball ($kind): refused, nothing left" test "$rc" -ne 0 -a ! -e "$home" -a ! -e "$work/evil"
  done
else
  echo "skip unsafe tarball tests: python3 is missing"
fi

# 10. the same install over a throwaway HTTP server on 127.0.0.1
if command -v python3 > /dev/null; then
  port=$((20000 + RANDOM % 20000))
  (cd "$fx" && python3 -m http.server "$port" --bind 127.0.0.1 > "$work/http.log" 2>&1) &
  server_pid=$!
  for _ in $(seq 1 50); do (exec 3<>"/dev/tcp/127.0.0.1/$port") 2> /dev/null && break; sleep 0.1; done
  home="$work/home10"
  PATH="$work/bin:$PATH" TMPDIR="$work/tmp" CIGSCRIPT_HOME="$home" CIGI_BOOTSTRAP_BASE_URL="http://127.0.0.1:$port" \
    sh "$script" --yes carton > "$work/stdout" 2> "$work/stderr"; rc=$?
  check "http on 127.0.0.1: installs" test "$rc" -eq 0 -a -f "$home/args.txt"
  check "http: the server saw one GET of the tarball and nothing else" test "$(grep -c '"GET ' "$work/http.log")" -ge 1 -a "$(grep '"GET ' "$work/http.log" | grep -vc "GET /ciginstaller-v$ver-linux-x86_64.tar.gz")" -eq 0
else
  echo "skip the http test: python3 is missing"
fi

# 11. the fill refuses a malformed sidecar and a missing platform
mkdir -p "$work/badfill"
cp "$fx"/ciginstaller-v$ver-linux-*.tar.gz "$work/badfill/"
echo "nothex  ciginstaller-v$ver-linux-x86_64.tar.gz" > "$work/badfill/ciginstaller-v$ver-linux-x86_64.tar.gz.sha256"
cp "$fx/ciginstaller-v$ver-linux-aarch64.tar.gz.sha256" "$work/badfill/"
bash "$here/ci/fill-bootstrap.sh" "$tag" "$repo" "$work/badfill" "$work/out-x" > /dev/null 2>&1; rc=$?
check "fill: a malformed sidecar fails" test "$rc" -ne 0
rm "$work/badfill/ciginstaller-v$ver-linux-aarch64.tar.gz.sha256"
cp "$fx/ciginstaller-v$ver-linux-x86_64.tar.gz.sha256" "$work/badfill/"
bash "$here/ci/fill-bootstrap.sh" "$tag" "$repo" "$work/badfill" "$work/out-y" > /dev/null 2>&1; rc=$?
check "fill: a missing Linux platform fails" test "$rc" -ne 0
check "fill: the published script's own checksum verifies" bash -c "cd '$work/out' && sha256sum -c install.sh.sha256 install.ps1.sha256 > /dev/null"
check "fill: windows is written as none until a build exists" grep -q "^\$ShaWindowsX86_64 = 'none'" "$work/out/install.ps1"

# 12. PowerShell: parse the filled script; on a non-Windows host it must refuse by name
if command -v pwsh > /dev/null; then
  # shellcheck disable=SC2016  # PowerShell's variables, not the shell's
  parse='$e=$null;$t=$null;[void][System.Management.Automation.Language.Parser]::ParseFile("'"$work/out/install.ps1"'",[ref]$t,[ref]$e);if($e){$e|ForEach-Object{$_.Message};exit 1}'
  check "install.ps1 parses" pwsh -NoProfile -Command "$parse"
  if [[ "$(uname -s)" != MINGW* && "$(uname -s)" != MSYS* ]]; then
    pwsh -NoProfile -File "$work/out/install.ps1" --yes carton > "$work/stdout" 2> "$work/stderr"; rc=$?
    check "install.ps1 refuses off Windows" test "$rc" -ne 0
    check "install.ps1 says so by name" said "this script is for Windows"
  fi
else
  echo "skip the PowerShell checks: pwsh is not installed (install.ps1 is untested here)"
fi

echo
if (( fails > 0 )); then echo "$fails check(s) failed"; exit 1; fi
echo "all bootstrap checks passed"
