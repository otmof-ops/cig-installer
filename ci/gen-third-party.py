#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
# SPDX-License-Identifier: FSL-1.1-ALv2
"""Regenerate THIRD-PARTY-NOTICES.md.

Same layout as CigScript's scripts/gen-third-party.py, but the rows come from `cargo metadata`
(no extra tool to install): every non-dev crate reachable from the two shipped binaries,
`ciginstaller` and `cigi-shim`, for the targets they release on. Pass --check to fail when
the file on disk is out of date (CI uses it).
"""
import collections, json, pathlib, subprocess, sys

root = pathlib.Path(__file__).resolve().parent.parent
SHIPPED = ("ciginstaller", "cigi-shim")
TARGETS = ("x86_64-unknown-linux-musl", "aarch64-unknown-linux-musl")


def metadata(target):
    out = subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1", "--locked", "--filter-platform", target],
        cwd=root,
    )
    return json.loads(out)


def linked(target):
    """(name, version) of every crate a release build of the shipped binaries links for `target`.

    `cargo tree -p` resolves features the way `cargo build -p ciginstaller -p cigi-shim` does,
    unlike `cargo metadata`, whose graph unifies features across the whole workspace (xtask turns
    on cigi-source's `tuf`, which the installer does not link)."""
    args = ["cargo", "tree", "--locked", "-e", "normal,build", "--target", target,
            "--prefix", "none", "--format", "{p}"]
    for name in sorted(SHIPPED):
        args += ["-p", name]
    out = subprocess.check_output(args, cwd=root, text=True)
    seen = set()
    for line in out.splitlines():
        parts = line.replace("(*)", "").split()
        if len(parts) >= 2 and parts[1].startswith("v"):
            seen.add((parts[0], parts[1][1:]))
    return seen


def shipped(meta, target):
    want = linked(target)
    return [p for p in meta["packages"] if p["source"] and (p["name"], p["version"]) in want]


def main():
    rows = {}
    for t in TARGETS:
        for p in shipped(metadata(t), t):
            rows[(p["name"], p["version"])] = p
    fam = collections.OrderedDict()
    for (name, ver), p in sorted(rows.items()):
        fam.setdefault(p.get("license") or "unrecorded", []).append(p)
    out = ["# Third-party notices", "",
           "ciginstaller and cigi-shim link the Rust crates below, pulled from crates.io as packages "
           "(nothing is vendored). Holder names come from each crate's metadata; `unrecorded` means the "
           "crate publishes no author field. Regenerate with `ci/gen-third-party.py` "
           "(it reads `cargo metadata`; dev-only crates are left out).", ""]
    for lic in sorted(fam):
        out += [f"## {lic}", "", "| crate | version | holder | source |", "|---|---|---|---|"]
        for p in fam[lic]:
            holder = ", ".join(p.get("authors") or []).replace("|", ", ") or "unrecorded"
            out.append(f"| {p['name']} | {p['version']} | {holder} | {p.get('repository') or 'crates.io'} |")
        out.append("")
    text = "\n".join(out)
    dest = root / "THIRD-PARTY-NOTICES.md"
    if "--check" in sys.argv:
        if not dest.exists() or dest.read_text() != text:
            sys.exit("THIRD-PARTY-NOTICES.md is out of date: run ci/gen-third-party.py")
        print("THIRD-PARTY-NOTICES.md is current")
        return
    dest.write_text(text)
    print(f"wrote THIRD-PARTY-NOTICES.md ({len(rows)} crates)")


main()
