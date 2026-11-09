#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
# SPDX-License-Identifier: Apache-2.0
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


def shipped(meta):
    pkgs = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    start = [i for i, p in pkgs.items() if p["name"] in SHIPPED]
    seen, todo = set(), list(start)
    while todo:
        i = todo.pop()
        if i in seen:
            continue
        seen.add(i)
        for d in nodes[i]["deps"]:
            # a dev-only edge is not linked into a release binary
            if any(k["kind"] != "dev" for k in d["dep_kinds"]):
                todo.append(d["pkg"])
    return [pkgs[i] for i in seen if pkgs[i]["source"]]  # path members have no source


def main():
    rows = {}
    for t in TARGETS:
        for p in shipped(metadata(t)):
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
