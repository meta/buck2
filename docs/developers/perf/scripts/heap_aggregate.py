#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""Aggregate a jemalloc heap dump by allocation site, with proper
per-stack scaling so the totals approximate jemalloc's reported
`allocated` bytes.

Usage:
  heap_aggregate.py <binary> <heap.prof> [--top N] [--ctx K] [--cum]

Why not jeprof: jeprof shells out to addr2line on every sampled
address. On a multi-GB Rust binary with split-DWARF references this
takes hours per profile. This script uses `nm --defined-only` (a few
seconds) plus binary search, then `rustfilt` if available for
demangling. See docs/developers/perf/memory.md for the math.

Flags:
  --top N      number of top sites to show (default 25)
  --ctx K      number of caller frames to show with each site (default 5)
  --cum        also show top-N "cumulative" (anywhere on the stack)
               in addition to the default flat (allocation site only)
  --no-skip-jemalloc
               do NOT skip jemalloc-internal frames when picking the
               leaf. By default we skip them because every heap-dump
               stack starts with jemalloc's own backtrace function.
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
    "GSS_C_NT_HOSTBASED_SERVICE",  # commonly aliased
    "_RNvCsfLfy6EI15iL_7___rustc11___rdl_alloc",
    "_RNvCsfLfy6EI15iL_7___rustc13___rdl_realloc",
}


def is_jemalloc(name):
    return (
        name.startswith(JEMALLOC_PREFIXES)
        or name in JEMALLOC_NAMES
        or "fallbackNewImpl" in name
    )


def load_symbols(binary):
    """Return (sorted list of (addr, name), parallel list of addrs)."""
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
    return syms, [s[0] for s in syms]


def make_lookup(syms, addrs):
    def lookup(addr):
        i = bisect.bisect_right(addrs, addr) - 1
        return syms[i][1] if i >= 0 else f"<unk 0x{addr:x}>"

    return lookup


def parse_heap(path):
    """Yield (count, bytes, [addresses]) per stack and return the period."""
    period = 524288
    stacks = []
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
                stacks.append((int(m.group(1)), int(m.group(2)), cur))
                cur = None
            elif line.startswith("MAPPED_LIBRARIES"):
                break
    return period, stacks


def scale_factor(count, bytes_, period):
    if count <= 0 or bytes_ <= 0:
        return 1.0
    avg = bytes_ / count
    p = 1 - math.exp(-avg / period)
    return 1.0 / p if p > 0 else 1.0


def find_demangler():
    """Return a callable name -> demangled, or identity if no demangler found."""
    # rustfilt handles Rust v0 mangling. c++filt does not, even with --format=rust.
    candidates = [
        os.environ.get("RUSTFILT"),
        "rustfilt",
        # fbsource location:
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
                    [_tool],
                    input="\n".join(names),
                    capture_output=True,
                    text=True,
                )
                # rustfilt emits a line per input
                return dict(zip(names, p.stdout.splitlines()))

            return demangle

    # Fall back to c++filt for C++ symbols and any Itanium-mangled Rust.
    def cppfilt(names):
        if not names:
            return {}
        p = subprocess.run(
            ["c++filt", "-n"],
            input="\n".join(names),
            capture_output=True,
            text=True,
        )
        return dict(zip(names, p.stdout.splitlines()))

    print(
        "warning: rustfilt not found; Rust v0 names will not be demangled",
        file=sys.stderr,
    )
    return cppfilt


def aggregate_stacks(stacks, period, lookup, skip_jemalloc, cum):
    flat_scaled = collections.Counter()
    flat_raw = collections.Counter()
    flat_stacks = collections.defaultdict(list)
    cum_scaled = collections.Counter()
    raw_total = 0
    scaled_total = 0.0

    for count, bytes_, addrs_in_stack in stacks:
        sf = scale_factor(count, bytes_, period)
        sb = bytes_ * sf
        raw_total += bytes_
        scaled_total += sb

        names = [lookup(a) for a in addrs_in_stack]
        if not skip_jemalloc:
            user = [n for n in names if not is_jemalloc(n)]
        else:
            user = names

        if user:
            leaf = user[0]
            flat_scaled[leaf] += sb
            flat_raw[leaf] += bytes_
            flat_stacks[leaf].append((sb, bytes_, count, user))

        if cum:
            seen = set()
            for n in names:
                if n not in seen:
                    seen.add(n)
                    cum_scaled[n] += sb

    return flat_scaled, flat_raw, flat_stacks, cum_scaled, raw_total, scaled_total


def print_flat(top_flat, flat_raw, flat_stacks, scaled_total, show, ctx):
    print(f"=== TOP {len(top_flat)} BY SCALED FLAT (allocation site) ===")
    print()
    for i, (leaf, sb) in enumerate(top_flat, 1):
        rb = flat_raw[leaf]
        scl = sb / rb if rb else 1.0
        pct = 100 * sb / scaled_total if scaled_total else 0
        print(
            f"#{i}  scaled={sb / 1e9:6.2f} GB  raw={rb / 1e9:6.2f} GB"
            f"  scale={scl:5.2f}×  ({pct:.1f}%)"
        )
        print(f"    LEAF: {show(leaf)[:200]}")
        site_stacks = sorted(flat_stacks[leaf], key=lambda x: -x[0])
        for j, (ssb, srb, sc, user) in enumerate(site_stacks[:2]):
            print(
                f"    └─ stack {j + 1} (scaled={ssb / 1e9:.2f} GB,"
                f" raw={srb / 1e6:.1f} MB, count={sc}):"
            )
            for k, frame in enumerate(user[: ctx + 1]):
                marker = "leaf" if k == 0 else f"  +{k}"
                print(f"       {marker}: {show(frame)[:200]}")
        print()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("binary")
    ap.add_argument("heap")
    ap.add_argument("--top", type=int, default=25)
    ap.add_argument("--ctx", type=int, default=5)
    ap.add_argument(
        "--cum", action="store_true", help="Also show cumulative (every frame on stack)"
    )
    ap.add_argument(
        "--no-skip-jemalloc",
        action="store_true",
        help="Do not skip jemalloc-internal frames when picking leaf",
    )
    args = ap.parse_args()

    print(f"loading symbols from {args.binary}...", file=sys.stderr)
    syms, addrs = load_symbols(args.binary)
    lookup = make_lookup(syms, addrs)
    print(f"  loaded {len(syms):,} symbols", file=sys.stderr)

    print(f"parsing {args.heap}...", file=sys.stderr)
    period, stacks = parse_heap(args.heap)
    print(f"  sample period: {period:,} bytes", file=sys.stderr)
    print(f"  {len(stacks):,} unique stacks", file=sys.stderr)

    flat_scaled, flat_raw, flat_stacks, cum_scaled, raw_total, scaled_total = (
        aggregate_stacks(stacks, period, lookup, args.no_skip_jemalloc, args.cum)
    )

    demangle = find_demangler()

    print()
    print(f"raw sampled bytes:    {raw_total:>15,}  ({raw_total / 2**30:.2f} GiB)")
    print(
        f"scaled (≈ allocated): {int(scaled_total):>15,}  ({scaled_total / 2**30:.2f} GiB)"
    )
    print()

    needed = set()
    top_flat = flat_scaled.most_common(args.top)
    for leaf, _ in top_flat:
        for _sb, _rb, _c, u in flat_stacks[leaf][:3]:
            for f in u[: args.ctx + 1]:
                needed.add(f)
    if args.cum:
        for n, _ in cum_scaled.most_common(args.top):
            needed.add(n)
    dm = demangle(list(needed))

    def show(name):
        return dm.get(name, name)

    print_flat(top_flat, flat_raw, flat_stacks, scaled_total, show, args.ctx)

    if args.cum:
        print(f"=== TOP {args.top} BY CUMULATIVE (anywhere on stack) ===")
        print()
        for n, sb in cum_scaled.most_common(args.top):
            pct = 100 * sb / scaled_total if scaled_total else 0
            print(f"  {sb / 1e9:6.2f} GB  ({pct:.1f}%)  {show(n)[:200]}")


if __name__ == "__main__":
    main()
