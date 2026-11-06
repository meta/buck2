#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""Print the full demangled call stacks where a specific leaf symbol
allocated.

Usage:
  heap_stacks.py <binary> <heap.prof> <leaf-substring> [N]

  <leaf-substring> matches against the *non-jemalloc* leaf frame
  (i.e. the first user frame) of each stack. Mangled or demangled
  substrings both work — matching is done before demangling.

  N: number of top stacks to print (by scaled bytes). Default 5.

Why: heap_aggregate.py only prints a few caller frames per site.
When the regression is in a deeply-recursive code path (DICE futures,
async traversal, etc.) you need the full stack to see what's going
on. This script prints every frame, demangled with rustfilt if
available.

See docs/developers/perf/memory_regression_hunting.md.
"""

import argparse
import bisect
import math
import os
import re
import shutil
import subprocess
import sys

JEMALLOC_PREFIXES = ("jemalloc_je_", "je_", "prof_", "tcache_", "_rjem_")
JEMALLOC_NAMES = {
    "prof_backtrace_impl",
    "malloc",
    "calloc",
    "realloc",
    "posix_memalign",
    "aligned_alloc",
    "valloc",
    "free",
    "do_rallocx",
    "mallocx",
    "rallocx",
    "GSS_C_NT_HOSTBASED_SERVICE",
    "_RNvCsfLfy6EI15iL_7___rustc11___rdl_alloc",
    "_RNvCsfLfy6EI15iL_7___rustc13___rdl_realloc",
}


def is_jemalloc(name):
    return (
        name.startswith(JEMALLOC_PREFIXES)
        or name in JEMALLOC_NAMES
        or "fallbackNewImpl" in name
    )


def find_demangler():
    candidates = [
        os.environ.get("RUSTFILT"),
        "rustfilt",
        os.path.expanduser("~/fbsource/third-party/rust/tools/rustfilt"),
        os.path.expanduser("~/fbsource2/third-party/rust/tools/rustfilt"),
    ]
    for c in candidates:
        if c and (os.path.isfile(c) or shutil.which(c)):
            tool = c

            def demangle(names, _tool=tool):
                if not names:
                    return {}
                p = subprocess.run(
                    [_tool], input="\n".join(names), capture_output=True, text=True
                )
                return dict(zip(names, p.stdout.splitlines()))

            return demangle
    print(
        "warning: rustfilt not found; Rust v0 names will not be demangled",
        file=sys.stderr,
    )

    def demangle(names):
        if not names:
            return {}
        p = subprocess.run(
            ["c++filt", "-n"], input="\n".join(names), capture_output=True, text=True
        )
        return dict(zip(names, p.stdout.splitlines()))

    return demangle


def load_symbols(binary):
    out = subprocess.run(
        ["nm", "-n", "--defined-only", binary],
        capture_output=True,
        text=True,
        check=True,
    ).stdout
    syms = []
    for line in out.splitlines():
        parts = line.split(maxsplit=2)
        if len(parts) < 3:
            continue
        try:
            a = int(parts[0], 16)
        except ValueError:
            continue
        syms.append((a, parts[2]))
    syms.sort()
    addrs = [s[0] for s in syms]

    def lookup(a):
        i = bisect.bisect_right(addrs, a) - 1
        return syms[i][1] if i >= 0 else f"<unk 0x{a:x}>"

    return lookup


def parse_heap_stacks(path, lookup):
    period = 524288
    candidate_leaves = set()
    raw_stacks = []
    with open(path) as f:
        first = f.readline().strip()
        m = re.match(r"heap_v2/(\d+)", first)
        if m:
            period = int(m.group(1))
        cur = None
        for line in f:
            if line.startswith("@ "):
                cur = [int(x, 16) for x in line[2:].split()]
            elif cur and line.lstrip().startswith("t*:"):
                m = re.match(r"\s*t\*:\s*(\d+):\s*(\d+)", line)
                if not m:
                    continue
                count, bytes_ = int(m.group(1)), int(m.group(2))
                names = [lookup(a) for a in cur]
                user = [n for n in names if not is_jemalloc(n)]
                if user:
                    candidate_leaves.add(user[0])
                raw_stacks.append((count, bytes_, names, user))
                cur = None
            elif line.startswith("MAPPED_LIBRARIES"):
                break
    return period, candidate_leaves, raw_stacks


def filter_matches(raw_stacks, leaf_substr, leaf_demangled, period):
    matches = []
    for count, bytes_, _names, user in raw_stacks:
        if not user:
            continue
        leaf = user[0]
        if leaf_substr not in leaf and leaf_substr not in leaf_demangled.get(
            leaf, leaf
        ):
            continue
        avg = bytes_ / count if count else 0
        p = 1 - math.exp(-avg / period) if avg else 1.0
        sf = 1.0 / p if p > 0 else 1.0
        matches.append((bytes_ * sf, bytes_, count, user))
    matches.sort(key=lambda x: -x[0])
    return matches


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("binary")
    ap.add_argument("heap")
    ap.add_argument("leaf", help="Substring of the leaf symbol (mangled OR demangled)")
    ap.add_argument("n", nargs="?", type=int, default=5)
    args = ap.parse_args()

    print(f"loading symbols from {args.binary}...", file=sys.stderr)
    lookup = load_symbols(args.binary)

    demangle = find_demangler()
    period, candidate_leaves, raw_stacks = parse_heap_stacks(args.heap, lookup)

    leaf_demangled = demangle(list(candidate_leaves))
    matches = filter_matches(raw_stacks, args.leaf, leaf_demangled, period)

    print(f"matched {len(matches)} stacks where leaf contains {args.leaf!r}")
    print(f"scaled total across matches: {sum(m[0] for m in matches) / 1e9:.2f} GB")
    print()

    needed = set()
    for _sb, _rb, _c, u in matches[: args.n]:
        for f in u:
            needed.add(f)
    dm = demangle(list(needed))

    for i, (sb, rb, c, u) in enumerate(matches[: args.n], 1):
        print(
            f"=== Stack #{i}: scaled={sb / 1e9:.3f} GB,"
            f" raw={rb / 1e6:.2f} MB, count={c}, depth={len(u)} user frames ==="
        )
        for j, fn in enumerate(u):
            print(f"  [{j:2}] {dm.get(fn, fn)}")
        print()


if __name__ == "__main__":
    main()
