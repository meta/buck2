#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""Decompose jemalloc's `active - allocated` gap (slab-internal
fragmentation) by size class.

`active - allocated` is the bytes jemalloc has committed into slabs but
that hold no live object: partially-empty slabs pinned by survivors.
Unlike `resident - active` (dirty/muzzy pages, a decay-policy problem)
this is a *placement* problem and the thing typed arenas / lifetime
segregation can fix.

This is the entry point of the fragmentation pipeline: it tells you
*which* size classes hold the waste, and for each one whether it is the
"few survivors pinning otherwise-dead slabs" shape (a good arena
candidate) or the "uniformly half-full" shape (segregate the transients
instead). To then attribute the waste in a bin to *allocation sites*, use
the sampling allocator + `experimental.utilization.batch_query` pipeline;
this script only needs the bin-level counters jemalloc already keeps.

Usage:
  bin_waste.py [stats.json]          # parse a saved stats dump
  bin_waste.py -                     # read stats JSON from stdin
  bin_waste.py --daemon              # run `buck2 debug allocator-stats` for you
  bin_waste.py --daemon --buck2 ./buck2 --isolation-dir v2

The JSON must include per-arena bin stats, i.e. NOT be produced with the
default `Jmdablxg` options (which suppress them). `--daemon` requests the
right options automatically; if you capture by hand use `-o J`:

  buck2 debug allocator-stats -o J > stats.json

Caveat (tcache): a region sitting in a per-thread tcache still counts in
`curregs`, so it looks live even though the application has freed it. That
*understates* the waste reported here. For a precise number, start the
daemon with `MALLOC_CONF=tcache:false`, or accept that the true waste is
somewhat higher than shown.
"""

import argparse
import json
import subprocess
import sys


def load_stats(args):
    """Return the parsed `jemalloc` stats object."""
    if args.daemon:
        cmd = [args.buck2]
        if args.isolation_dir:
            cmd += ["--isolation-dir", args.isolation_dir]
        cmd += ["debug", "allocator-stats", "-o", "J"]
        out = subprocess.run(cmd, capture_output=True, text=True, check=True).stdout
        doc = json.loads(out)
    elif args.stats == "-":
        doc = json.load(sys.stdin)
    else:
        with open(args.stats) as f:
            doc = json.load(f)
    return doc["jemalloc"]


def merged_bins(je):
    """Return per-bin stat dicts merged across all arenas.

    Prefers jemalloc's own `merged` arena; falls back to summing the
    numeric-keyed per-arena entries if merging was suppressed.
    """
    sa = je["stats.arenas"]
    if "merged" in sa:
        return sa["merged"]["bins"]

    nbins = je["arenas"]["nbins"]
    acc = [None] * nbins
    for key, arena in sa.items():
        if not key.isdigit():
            continue
        for j, b in enumerate(arena["bins"]):
            if acc[j] is None:
                acc[j] = dict.fromkeys(("curregs", "curslabs", "nonfull_slabs"), 0)
            for k in acc[j]:
                acc[j][k] += b[k]
    return acc


def bin_rows(je):
    """Yield a derived metrics dict per size-class bin, with waste computed."""
    cfg = je["arenas"]["bin"]
    bins = merged_bins(je)
    rows = []
    for j, b in enumerate(bins):
        size = cfg[j]["size"]
        nregs = cfg[j]["nregs"]
        slab_size = cfg[j]["slab_size"]
        curregs = b["curregs"]
        curslabs = b["curslabs"]
        nonfull = b["nonfull_slabs"]

        allocated = curregs * size
        active = curslabs * slab_size
        waste = active - allocated

        # Of the slabs that have free space, how many live objects pin each
        # one on average. Low (~1-3) => an arena / pool that segregates this
        # type by lifetime can free whole slabs cheaply. High (near nregs)
        # => the slabs are genuinely full of survivors; arenas won't help and
        # the win, if any, is segregating the *transients* in this bin.
        full_slabs = curslabs - nonfull
        regs_in_nonfull = max(0, curregs - full_slabs * nregs)
        live_per_nonfull = regs_in_nonfull / nonfull if nonfull else 0.0

        rows.append(
            {
                "size": size,
                "nregs": nregs,
                "slab_size": slab_size,
                "curregs": curregs,
                "curslabs": curslabs,
                "nonfull": nonfull,
                "allocated": allocated,
                "active": active,
                "waste": waste,
                "util": (allocated / active) if active else 1.0,
                "live_per_nonfull": live_per_nonfull,
            }
        )
    return rows


def gib(n):
    return n / 2**30


def mib(n):
    return n / 2**20


def print_report(je, rows, top):
    s = je["stats"]
    allocated, active, resident = s["allocated"], s["active"], s["resident"]
    aa = active - allocated
    small_waste = sum(r["waste"] for r in rows)
    merged = je["stats.arenas"].get("merged", {})
    tcache_bytes = merged.get("tcache_bytes", 0)

    print("=== global ===")
    print(f"  allocated         {allocated:>16,}  ({gib(allocated):.2f} GiB)")
    print(f"  active            {active:>16,}  ({gib(active):.2f} GiB)")
    print(f"  resident          {resident:>16,}  ({gib(resident):.2f} GiB)")
    print(
        f"  active-allocated  {aa:>16,}  ({gib(aa):.2f} GiB, "
        f"{100 * aa / active if active else 0.0:.1f}% of active)   <- slab fragmentation"
    )
    print(
        f"  resident-active   {resident - active:>16,}  ({gib(resident - active):.2f} GiB)"
        "   <- dirty/muzzy pages (decay policy, not this script)"
    )
    print()
    print(
        f"  small-bin waste   {small_waste:>16,}  ({gib(small_waste):.2f} GiB, "
        f"{100 * small_waste / aa if aa else 0.0:.0f}% of active-allocated)"
    )
    print(
        f"  other (large/rounding) {aa - small_waste:>11,}  ({gib(aa - small_waste):.2f} GiB)"
    )
    if tcache_bytes:
        print(
            f"  tcache_bytes      {tcache_bytes:>16,}  ({mib(tcache_bytes):.0f} MiB)"
            "   <- counted as live; understates waste above"
        )
    print()

    rows = sorted(rows, key=lambda r: -r["waste"])[:top]
    print(f"=== top {len(rows)} size classes by waste ===")
    print(
        f"{'size':>6} {'util%':>6} {'waste':>9} {'alloc':>9} "
        f"{'curslabs':>10} {'nonfull':>10} {'regs':>5} {'live/nonfull':>12}  hint"
    )
    for r in rows:
        # Bin counters can only suggest the regime, not prove it: they give
        # the *mean* fill of nonfull slabs, not the per-slab cohort structure
        # that decides whether an arena actually frees whole slabs. Treat
        # these as hypotheses to confirm with the sampling stage.
        fill_frac = r["live_per_nonfull"] / r["nregs"] if r["nregs"] else 1.0
        if r["util"] >= 0.6:
            hint = ""
        elif fill_frac < 0.25:
            hint = "sparse -> arena?"
        else:
            hint = "half-full -> segregate transients?"
        print(
            f"{r['size']:>6} {100 * r['util']:>6.1f} "
            f"{mib(r['waste']):>7.1f}MB {mib(r['allocated']):>7.1f}MB "
            f"{r['curslabs']:>10,} {r['nonfull']:>10,} {r['nregs']:>5} "
            f"{r['live_per_nonfull']:>12.1f}  {hint}"
        )
    print()
    print(
        "live/nonfull = mean live objects pinning each partially-empty slab.\n"
        "  low  => few survivors hold dead slabs open: typed arena / lifetime pool wins.\n"
        "  high => slabs genuinely full: segregate the transients in this bin instead.\n"
        "Next: attribute a bin's waste to allocation sites with the sampling +\n"
        "experimental.utilization.batch_query pipeline (see mem_frag/)."
    )


def main():
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument(
        "stats",
        nargs="?",
        default="-",
        help="stats JSON file, or '-' for stdin (default)",
    )
    ap.add_argument(
        "--daemon",
        action="store_true",
        help="run `buck2 debug allocator-stats -o J` to capture stats",
    )
    ap.add_argument("--buck2", default="buck2", help="buck2 binary for --daemon")
    ap.add_argument("--isolation-dir", default=None, help="isolation dir for --daemon")
    ap.add_argument("--top", type=int, default=20, help="size classes to show")
    args = ap.parse_args()

    je = load_stats(args)
    rows = bin_rows(je)
    print_report(je, rows, args.top)


if __name__ == "__main__":
    main()
