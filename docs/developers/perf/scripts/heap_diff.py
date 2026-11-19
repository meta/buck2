#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""Diff two jemalloc heap dumps from (potentially different) buck2
binaries. Aggregates each by allocation site with proper per-stack
scaling and prints the top growers and shrinkers.

Usage:
  heap_diff.py <bin_a> <heap_a> <bin_b> <heap_b> [<label_a> <label_b>] [--top N]

The two binaries can differ — the script symbolicates each profile
against its own binary, so a regression introduced in code reorganization
will still align by symbol name (Rust symbols are name-stable across
unrelated binary changes).

See docs/developers/perf/memory_regression_hunting.md for the workflow.
"""

import argparse
import bisect
import collections
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


def load_lookup(binary):
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

    def lookup(addr):
        i = bisect.bisect_right(addrs, addr) - 1
        return syms[i][1] if i >= 0 else f"<unk 0x{addr:x}>"

    return lookup


def aggregate(binary, heap):
    lookup = load_lookup(binary)
    period = 524288
    flat = collections.Counter()
    total = 0.0
    with open(heap) as f:
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
                avg = bytes_ / count if count else 0
                p = 1 - math.exp(-avg / period) if avg else 1.0
                sf = 1.0 / p if p > 0 else 1.0
                sb = bytes_ * sf
                total += sb
                user = [n for n in (lookup(a) for a in cur) if not is_jemalloc(n)]
                if user:
                    flat[user[0]] += sb
                cur = None
            elif line.startswith("MAPPED_LIBRARIES"):
                break
    return flat, total


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

            def f(names, _tool=tool):
                if not names:
                    return {}
                p = subprocess.run(
                    [_tool], input="\n".join(names), capture_output=True, text=True
                )
                return dict(zip(names, p.stdout.splitlines()))

            return f
    print(
        "warning: rustfilt not found; Rust v0 names will not be demangled",
        file=sys.stderr,
    )

    def f(names):
        if not names:
            return {}
        p = subprocess.run(
            ["c++filt", "-n"], input="\n".join(names), capture_output=True, text=True
        )
        return dict(zip(names, p.stdout.splitlines()))

    return f


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("bin_a")
    ap.add_argument("heap_a")
    ap.add_argument("bin_b")
    ap.add_argument("heap_b")
    ap.add_argument("label_a", nargs="?", default="A")
    ap.add_argument("label_b", nargs="?", default="B")
    ap.add_argument("--top", type=int, default=25)
    args = ap.parse_args()

    print(f"aggregating {args.label_a}...", file=sys.stderr)
    fa, ta = aggregate(args.bin_a, args.heap_a)
    print(f"aggregating {args.label_b}...", file=sys.stderr)
    fb, tb = aggregate(args.bin_b, args.heap_b)

    print()
    print(f"{args.label_a} scaled total: {ta / 2**30:.2f} GiB")
    print(f"{args.label_b} scaled total: {tb / 2**30:.2f} GiB")
    print(
        f"delta:                   {(tb - ta) / 2**30:+.2f} GiB"
        f"  ({100 * (tb - ta) / ta:+.2f}% of {args.label_a})"
    )

    keys = set(fa) | set(fb)
    diffs = [(k, fb.get(k, 0) - fa.get(k, 0), fa.get(k, 0), fb.get(k, 0)) for k in keys]
    diffs.sort(key=lambda x: -x[1])
    growers = [d for d in diffs if d[1] > 0][: args.top]
    shrinkers = sorted(
        (d for d in diffs if d[1] < 0),
        key=lambda x: x[1],
    )[: args.top]

    needed = [k for k, _, _, _ in growers + shrinkers]
    dm = find_demangler()(needed)

    def show(name):
        return dm.get(name, name)

    print()
    print(f"=== TOP {args.top} GROWERS ({args.label_b} − {args.label_a}) ===")
    print(f"{'delta':>10} {args.label_a:>10} {args.label_b:>10}  symbol")
    for k, d, a, b in growers:
        print(f"{d / 1e9:>+9.2f}G {a / 1e9:>9.2f}G {b / 1e9:>9.2f}G  {show(k)[:160]}")

    print()
    print(f"=== TOP {args.top} SHRINKERS ({args.label_b} − {args.label_a}) ===")
    print(f"{'delta':>10} {args.label_a:>10} {args.label_b:>10}  symbol")
    for k, d, a, b in shrinkers:
        print(f"{d / 1e9:>+9.2f}G {a / 1e9:>9.2f}G {b / 1e9:>9.2f}G  {show(k)[:160]}")


if __name__ == "__main__":
    main()
