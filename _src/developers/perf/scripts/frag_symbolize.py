#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""Symbolize a raw fragmentation-attribution dump from the mem_frag profiler.

The daemon dumps raw instruction pointers (in-process symbolization via
gimli/addr2line takes minutes on buck2's multi-GB binary). This resolves them
offline with `nm` + binary search, the same fast path heap_aggregate.py uses.

Two dump formats are auto-detected:
 - the default cross-class attribution (size-class table + top sites by waste);
 - a focus dump (`BUCK2_MEMFRAG_FOCUS=<size>`), which captured one size class at
   rate 1 and gets the exact cohort/mixed split and per-slab leaf mixtures.

Usage:
  frag_symbolize.py <binary> <dump>      # dump from $BUCK2_MEMFRAG_TRIGGER.<pid>.out
  frag_symbolize.py <binary> - < dump
  frag_symbolize.py <binary> <dump> --top 30 --ctx 6

By default symbols are shortened to `crate~Type` (middle modules replaced with
`~`, crate and final name kept) so the type/method survives; pass --full for the
complete symbols, no eliding or truncation.

buck2 is a non-PIE executable, so the raw runtime addresses match `nm`
addresses directly (no load-base adjustment), as for jemalloc heap dumps.
"""

import argparse
import bisect
import os
import re
import shutil
import subprocess
import sys

# A `::`-separated Rust path of >=2 components (identifiers only, so it stops at
# `<`, `>`, `,`, spaces, `{`, etc. — generic args are matched separately).
_PATH_RE = re.compile(r"[A-Za-z_][A-Za-z0-9_]*(?:::[A-Za-z_][A-Za-z0-9_]*)+")


def _shorten_one(m):
    parts = m.group(0).split("::")
    if len(parts) <= 2:
        return m.group(0)
    return f"{parts[0]}~{parts[-1]}"


def shorten(name, full=False):
    """Collapse `crate::mod1::mod2::Type` to `crate~Type` (keep the crate and the
    final name, replace the middle modules with `~`) in every path of the symbol,
    including paths nested inside generics. `full=True` returns it unchanged."""
    return name if full else _PATH_RE.sub(_shorten_one, name)


# Frames that are allocator plumbing or the profiler itself, skipped when
# picking the leaf (cf. JEMALLOC_* in heap_aggregate.py).
SKIP_SUBSTRINGS = (
    "mem_frag",
    "backtrace",
    "GlobalAlloc",
    "__rust_alloc",
    "__rg_alloc",
    "__rdl_alloc",
    "alloc_impl",
    "box_new",
    "raw_vec",
    "5alloc",  # the `alloc` crate in mangled v0 names
    "je_",
    "jemalloc_je_",
    "tcache_",
    "_rjem_",
)


def is_internal(name):
    return any(s in name for s in SKIP_SUBSTRINGS)


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
    return syms, [s[0] for s in syms]


def make_lookup(syms, addrs):
    def lookup(addr):
        i = bisect.bisect_right(addrs, addr) - 1
        return syms[i][1] if i >= 0 else f"<unk 0x{addr:x}>"

    return lookup


def find_demangler():
    candidates = [
        os.environ.get("RUSTFILT"),
        "rustfilt",
        os.path.expanduser("~/fbsource/third-party/rust/tools/rustfilt"),
    ]
    for c in candidates:
        if c and (os.path.isfile(c) or shutil.which(c)):

            def demangle(names, _tool=c):
                if not names:
                    return {}
                p = subprocess.run(
                    [_tool], input="\n".join(names), capture_output=True, text=True
                )
                return dict(zip(names, p.stdout.splitlines()))

            return demangle

    print("warning: rustfilt not found; names left mangled", file=sys.stderr)
    return lambda names: {n: n for n in names}


def parse_dump(lines):
    """Return (header_lines, size_rows, stacks) where each stack is
    (waste, samples, [addrs])."""
    header, size_rows, stacks = [], [], []
    for line in lines:
        line = line.rstrip("\n")
        if line.startswith("STACK "):
            fields = line.split()
            waste = float(fields[1].split("=")[1])
            samples = int(fields[2].split("=")[1])
            addrs = [int(x, 16) for x in fields[3:]]
            stacks.append((waste, samples, addrs))
        elif line.startswith("SIZE "):
            size_rows.append(line)
        else:
            header.append(line)
    return header, size_rows, stacks


def leaf_index(addrs, lookup):
    for i, a in enumerate(addrs):
        if not is_internal(lookup(a)):
            return i
    return 0


def parse_focus(lines):
    """Parse a focus dump: returns (header_kv, {stack_id: [addrs]}, comps) where
    each comp is (slabs, regions, [stack_ids])."""
    head, stacks, comps = {}, {}, []
    for line in lines:
        parts = line.split()
        if not parts:
            continue
        if parts[0] == "memfrag_focus_v1":
            head = dict(p.split("=") for p in parts[1:])
        elif parts[0] == "STACK":
            stacks[int(parts[1])] = [int(x, 16) for x in parts[2:]]
        elif parts[0] == "COMP":
            kv = {}
            ids = []
            for p in parts[1:]:
                if "=" in p:
                    k, v = p.split("=")
                    kv[k] = v
                else:
                    ids.append(int(p))
            comps.append((int(kv["slabs"]), int(kv["regions"]), ids))
    return head, stacks, comps


def render_focus(head, stacks, comps, lookup, top, full):
    """Symbolize a focus dump: collapse each slab's stacks to their leaf
    functions and report the exact cohort/mixed split and leaf mixtures."""
    nregs = int(head["nregs"])
    groups = int(head["groups"])
    live = int(head["live_regions"])
    unit = "slab" if head.get("single_page") == "1" else "page"
    mean_live = live / groups if groups else 0.0

    # stack id -> demangled leaf name.
    leaf_addr = {sid: a[leaf_index(a, lookup)] if a else 0 for sid, a in stacks.items()}
    dm = find_demangler()(list({lookup(a) for a in leaf_addr.values()}))

    def leaf_name(sid):
        return dm.get(lookup(leaf_addr[sid]), lookup(leaf_addr[sid]))

    # Re-aggregate compositions by their *leaf* set (distinct stack ids can share
    # a leaf, so a slab with two such ids is still a single-leaf cohort).
    by_leafset = {}  # frozenset(leaves) -> [slabs, regions]
    per_leaf = {}  # leaf -> [slabs_total, slabs_sole]
    for slabs, regions, ids in comps:
        leaves = frozenset(leaf_name(i) for i in ids)
        e = by_leafset.setdefault(leaves, [0, 0])
        e[0] += slabs
        e[1] += regions
        for lf in leaves:
            pl = per_leaf.setdefault(lf, [0, 0])
            pl[0] += slabs
            if len(leaves) == 1:
                pl[1] += slabs

    cohort = sum(s for ls, (s, _) in by_leafset.items() if len(ls) == 1)
    mixed = sum(s for ls, (s, _) in by_leafset.items() if len(ls) > 1)

    # Fill is a ratio, so compute it from totals (grouping-independent): it is
    # exact and comparable to the size-class table. Do NOT divide a per-page mean
    # by the per-slab nregs for multi-page classes — that understates it ~Nx.
    class_sz = int(head["class"])
    waste = float(head["waste"])
    alloc_b = live * class_sz
    fill = alloc_b / (alloc_b + waste) if alloc_b + waste else 0.0
    surv_per_slab = fill * nregs  # exact mean live per slab (= fill × regs/slab)

    print(
        f"=== focus: size class {class_sz} "
        f"({nregs} regs/slab, {'single' if unit == 'slab' else 'multi'}-page) ==="
    )
    print(
        f"  {unit}s={groups}  live_regions={live}  surv/slab={surv_per_slab:.1f}  "
        f"fill={100 * fill:.1f}%  waste={waste / 2**20:.1f}MB"
        + (f"  (mean_live/page={mean_live:.1f})" if unit == "page" else "")
    )
    tot = cohort + mixed
    print(
        f"  cohort {unit}s={cohort} ({100 * cohort / tot:.1f}%)  "
        f"mixed {unit}s={mixed} ({100 * mixed / tot:.1f}%)  "
        f"[exact — rate-1 capture of this class]"
    )
    print()

    print(f"=== top {min(top, len(by_leafset))} leaf mixtures by {unit} count ===")
    for leaves, (slabs, _regions) in sorted(
        by_leafset.items(), key=lambda kv: -kv[1][0]
    )[:top]:
        tag = "cohort" if len(leaves) == 1 else f"mix×{len(leaves)}"
        names = " + ".join(sorted(shorten(n, full) for n in leaves))
        print(f"  {slabs:>7} {unit}s ({100 * slabs / tot:>4.1f}%) {tag:>7}: {names}")
    print()

    print(f"=== leaves by {unit}s pinned (sole = pure-cohort {unit}s) ===")
    for lf, (tot_s, sole_s) in sorted(per_leaf.items(), key=lambda kv: -kv[1][0])[:top]:
        print(f"  {tot_s:>7} {unit}s ({sole_s:>7} sole)  {shorten(lf, full)}")


def main():
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("binary")
    ap.add_argument("dump")
    ap.add_argument("--top", type=int, default=30)
    ap.add_argument("--ctx", type=int, default=6)
    ap.add_argument(
        "--full",
        action="store_true",
        help="print full symbols (no `crate~Type` eliding, no truncation)",
    )
    args = ap.parse_args()

    print(f"loading symbols from {args.binary}...", file=sys.stderr)
    syms, addrs = load_symbols(args.binary)
    lookup = make_lookup(syms, addrs)
    print(f"  {len(syms):,} symbols", file=sys.stderr)

    # Read the whole dump once; stdin can only be consumed a single time, so we
    # can't sniff the format and then re-open it. Focus dumps (one class, exact
    # composition) have a different layout, detected from the first line.
    if args.dump == "-":
        lines = sys.stdin.readlines()
    else:
        with open(args.dump) as fh:
            lines = fh.readlines()
    if lines and lines[0].startswith("memfrag_focus_v1"):
        head, fstacks, comps = parse_focus(lines)
        render_focus(head, fstacks, comps, lookup, args.top, args.full)
        return

    header, size_rows, stacks = parse_dump(lines)
    stacks.sort(key=lambda s: -s[0])
    total = sum(s[0] for s in stacks) or 1.0

    print("\n".join(header))
    print()
    print("=== waste by size class ===")
    print(f"{'size':>6} {'waste':>10} {'samples':>8} {'surv/slab':>9} {'fill%':>6}")
    for row in size_rows:
        parts = row.split()
        size = parts[1]
        kv = dict(p.split("=") for p in parts[2:])
        # surv/slab and fill% are exact for every class (new format only); old
        # reports lack them.
        if "surv_per_slab" in kv:
            surv = float(kv["surv_per_slab"])
            nregs = int(kv["nregs"])
            surv_s = f"{surv:.1f}"
            fill_s = f"{100 * surv / nregs:.1f}" if nregs else "-"
        else:
            surv_s, fill_s = "-", "-"
        print(
            f"{size:>6} {float(kv['waste']) / 2**20:>8.1f}MB {kv['samples']:>8} "
            f"{surv_s:>9} {fill_s:>6}"
        )
    print(
        "(exact cohort/mixed composition per class: rerun with "
        "BUCK2_MEMFRAG_FOCUS=<size>)"
    )
    print()

    # Cumulative by leaf: the same constructor shows up under many per-rule
    # stacks, each individually small; collapsing by leaf surfaces the real
    # offender. The leaf is the first non-internal frame.
    cum = {}
    for waste, samples, a in stacks:
        leaf = lookup(a[leaf_index(a, lookup)]) if a else "<empty>"
        c = cum.setdefault(leaf, [0.0, 0])
        c[0] += waste
        c[1] += samples
    top_cum = sorted(cum.items(), key=lambda kv: -kv[1][0])[: args.top]

    top = stacks[: args.top]
    needed = {name for name, _ in top_cum}
    for _w, _s, a in top:
        for x in a[: args.ctx + 8]:
            needed.add(lookup(x))
    dm = find_demangler()(list(needed))

    def show(addr):
        return dm.get(lookup(addr), lookup(addr))

    print(f"=== top {len(top_cum)} leaf functions by pinned waste (cumulative) ===")
    for name, (waste, samples) in top_cum:
        print(
            f"  {waste / 2**20:>8.1f} MB ({100 * waste / total:>4.1f}%)"
            f"  {samples:>7} samples  {shorten(dm.get(name, name), args.full)}"
        )
    print()

    print(f"=== top {len(top)} sites by pinned waste ===")
    for i, (waste, samples, a) in enumerate(top, 1):
        print(
            f"#{i:<3} {waste / 2**20:>8.1f} MB ({100 * waste / total:>4.1f}%)"
            f"  {samples} samples"
        )
        leaf = leaf_index(a, lookup)
        for j, addr in enumerate(a[leaf : leaf + args.ctx + 1]):
            marker = "leaf" if j == 0 else "  ^ "
            print(f"     {marker}: {shorten(show(addr), args.full)}")


if __name__ == "__main__":
    main()
