---
id: perf_memory_fragmentation
title: Fragmentation Attribution
---

This page is about the `active - allocated` gap specifically — jemalloc
small-bin **slab fragmentation** — and how to attribute it back to the
allocation sites that cause it. For general allocator-stats and heap profiling
see [memory.md](memory.md).

## Decompose first

"Fragmentation" is three different problems; only one needs the tooling here.
Split the gap before doing anything else:

- **`resident - active`** — dirty/muzzy pages jemalloc hasn't returned to the
  OS. A decay-policy problem (`dirty_decay_ms`, `background_thread`, an explicit
  `arena.MALLCTL_ARENAS_ALL.purge`), not placement. Watch out for THP: with
  hugepages one live 4 KB region can pin 2 MB resident.
- **`active - allocated`** — slabs committed but not full: partially-empty slabs
  pinned by long-lived survivors. This is the placement problem typed arenas /
  lifetime segregation fix, and the subject of this page.
- **`allocated - requested`** — size-class rounding. jemalloc doesn't track
  requested bytes; a shim comparing `nallocx` to the requested size measures it.

On a representative daemon today `active - allocated` is ~2.7 GiB (≈22% of
active), essentially **all** of it small-bin slab waste.

## Step 1 — which size classes (`bin_waste.py`)

[`scripts/bin_waste.py`](scripts/bin_waste.py) parses jemalloc's per-bin
counters and ranks size classes by `active - allocated`:

```sh
buck2 debug allocator-stats -o J > stats.json   # -o J: do NOT suppress bin stats
scripts/bin_waste.py stats.json
# or: scripts/bin_waste.py --daemon
```

For each bin it prints utilization, total waste, and `live/nonfull` — the mean
number of live objects pinning each partially-empty slab. That number sorts the
bins into two regimes:

- **sparse** (few survivors per slab): a typed arena / lifetime pool for this
  type can free whole slabs. Arena candidate.
- **half-full** (slabs genuinely full of survivors): an arena won't help;
  segregate the *transients* sharing the bin instead.

This is a hypothesis, not proof — bin counters give the mean fill, not the
per-slab cohort structure. Step 2 confirms it and names the sites.

Note the **tcache caveat**: a region cached in a per-thread tcache still counts
as live (`curregs`), so the reported waste is a lower bound. For a precise
number start the daemon with `MALLOC_CONF=tcache:false`.

## Step 2 — which allocation sites (`facebook/mem_frag/`)

Knowing the bin isn't enough; you need the call sites whose survivors pin its
slabs. [`facebook/mem_frag/`](../../../facebook/mem_frag/) does this with a
sampling global allocator plus `experimental.utilization.batch_query`:

1. `SamplingAlloc` (a `GlobalAlloc` wrapping the system allocator) records a
   count-weighted sample of `(ptr, size, stack)` for live allocations, removing
   entries on free. This is deliberately **not** jemalloc's `prof`: `prof`
   promotes sampled small allocations to dedicated extents, which perturbs
   placement and makes utilization queries on them meaningless.
2. At a fragmented steady state, batch-query `experimental.utilization` for
   every live sampled pointer. For each it returns the slab's `nfree`, `nregs`,
   and extent `size`; the region size is `size / nregs`.
3. Attribute each slab's waste to the survivors pinning it.

Each sampled live pointer contributes `nfree·region_size / (r·live)` to its
stack's total, an unbiased estimate of the per-stack pinned waste (`r =
1/2^shift`). The full derivation, and why count-weighted sampling is what makes
it unbiased, is in the `mem_frag::attribute` module docs — that's where you'll
be when changing it, so it lives next to the code rather than here.

### Fill: how sparsely are the slabs filled

Per size class the report gives two columns that are *exact* (not rate-limited):
**surv/slab** (mean live objects per slab) and **fill%** (`surv/slab ÷
regions-per-slab`). Each sample carries its slab's exact `live = nregs − nfree`;
a fuller slab just yields proportionally more samples, so weighting each sample
by `1/live` cancels that size bias and recovers the mean over slabs without
needing slab identity (so it works for multi-page classes too). A low fill% on a
high-waste class is the arena/pool signal: the slabs are mostly empty, pinned by
a few survivors.

### Cohort vs mixed (focus mode)

Fill tells you the slabs are sparse; the next question is whether the survivors
in a slab are *one cohort* (→ an arena for that site frees whole slabs) or *many
independent sources* (→ you'd have to evict them all; segregate the transients
instead). This needs the per-slab composition, and **shift-based sampling cannot
give it**: at any real rate most slabs contribute a single sample, so almost
every slab looks like a one-stack "cohort" (the tell is `mixed ≈ samples − 1`).
Reporting cohort/mixed from sampled data is actively misleading, so the
size-class table does not.

Instead, **focus mode** captures *one* size class at rate 1
(`BUCK2_MEMFRAG_FOCUS=<size>`, nothing else sampled, so the side table stays
bounded). Every region of that class is observed, so the cohort/mixed split and
the per-slab *leaf mixture* are exact. `frag_symbolize.py` auto-detects a focus
dump and prints: the exact cohort vs mixed fraction; the top leaf-*sets* by slab
count (e.g. "N slabs = {A}", "M slabs = {A, B, C}"); and per leaf how many slabs
it pins versus how many it is the *sole* occupant of. The last is what tells you
whether an arena for one site pays, or whether a family of co-allocated types
must move together.

Grouping is by 4 KiB page: for a single-page class that is the exact slab; a
multi-page class is grouped per page (the right unit for page-level reclaim — an
all-free page can be swapped out / `MADV_FREE`d even when its slab is not
returned; the exact multi-page slab base isn't recoverable from userspace, see
the caveats).

### Validating it

[`facebook/mem_frag:harness`](../../../facebook/mem_frag/main.rs) builds a known
lifetime-mixing steady state (sparse survivors across otherwise-dead slabs, plus
a dense control that allocates a lot but wastes nothing) and runs the
attribution:

```sh
buck2 run fbcode//buck2/facebook/mem_frag:harness
```

It samples at rate 1 (exact) and additionally re-derives the total from a 1/64
subsample, demonstrating the estimator converges. Expected: the sparse sites top
the ranking, the dense control gets ~0, surv/slab is 1 for the 256 class and 8
for the 96 class, and the subsample total lands within a few percent of exact.
`BUCK2_MEMFRAG_FOCUS=256 buck2 run …` exercises focus mode (expect all-cohort).

### Caveats

- **Rust-only.** A Rust `#[global_allocator]` sees Rust allocations, not C++
  ones (folly etc. call `malloc` directly). The slab `nfree`/`nregs` are
  ground-truth across both, so C++ survivors in a slab are correctly left
  *unattributed* — the gap between summed estimates and total is real, not error.
- **tcache.** Per-thread tcaches hold freed-but-cached regions; the dump flushes
  the calling thread only. Use `MALLOC_CONF=tcache:false` for a precise
  process-wide snapshot.
- **Cohort/mixed needs focus mode.** Shift-based sampling can't characterize
  per-slab composition (most slabs get one sample → look like cohorts), so the
  size-class table reports only the exact surv/slab and fill%. Use focus mode for
  cohort/mixed. Focus overhead is real: it backtraces *every* allocation in the
  class at rate 1, so on a hot class it can slow a workload several-fold — pick a
  smaller workload when iterating.
- **Multi-page slab base.** Focus composition groups by 4 KiB page; for a
  multi-page class the page (not the slab) is the unit. The exact multi-page slab
  base is *not recoverable from userspace*: jemalloc exposes it only through
  header-inline functions (`emap_edata_lookup`, `edata_base_get`) over internal
  structs, not an exported API. Per-page is both the best available and the right
  unit for page-level reclaim.

## Step 3 — running against the real daemon

`SamplingAlloc` is wired into the daemon behind `-c buck2_dev.memfrag=true` (off by
default; normal builds are byte-for-byte unaffected). `bin/buck2.rs` calls
`mem_frag::init()` at process start to enable sampling *before* the daemonizing
fork (the side table must exist when fork copies the heap), and
`buck2_daemon` calls `mem_frag::start_dump_watcher()` *after* daemonizing —
buck2 double-forks and threads do not survive fork, so a watcher started at
entry would be lost.

```sh
# Build a profiling buck2 (opt, so jemalloc + its mallctls are linked; dev/dbgo
# do not use jemalloc). Build in one checkout, run daemons in another.
buck2 build @fbcode//mode/opt fbcode//buck2:buck2 -c buck2_dev.memfrag=true --out /tmp/b2

# Start the daemon with sampling on. tcache:false makes the utilization query
# exact; shift N samples 1/2^N of allocations (6-10 is a good range).
cd <other-checkout>
env MALLOC_CONF=tcache:false BUCK2_MEMFRAG_SHIFT=6 \
    BUCK2_MEMFRAG_TRIGGER=/tmp/memfrag /tmp/b2 cquery 'deps(//foo/...)'  # warm up

# Trigger a dump (writes a raw report per process; pick the daemon's pid).
touch /tmp/memfrag; sleep 5; rm /tmp/memfrag
DPID=$(/tmp/b2 status | python3 -c 'import sys,json;print(json.load(sys.stdin)["process_info"]["pid"])')
scripts/frag_symbolize.py /tmp/b2 /tmp/memfrag.$DPID.out --top 20

# For the exact cohort/mixed composition of a hot class (e.g. 192), restart the
# daemon in focus mode and re-run the workload, then trigger + symbolize as
# above (frag_symbolize auto-detects the focus dump):
env MALLOC_CONF=tcache:false BUCK2_MEMFRAG_FOCUS=192 \
    BUCK2_MEMFRAG_TRIGGER=/tmp/memfrag /tmp/b2 cquery 'deps(//foo/...)'
```

Cross-check the attributed total against `bin_waste.py` for the same daemon;
they measure the same `active - allocated` from opposite ends (bin counters vs.
sampled sites) and should agree to within sampling error.

### Worked example

On a daemon after a `cquery` over buck2's own graph (measured before the attr
arena existed), the ~1.2 GiB of small-bin waste attributed almost entirely to
**`TargetNode` construction during load/coerce** (cumulative by leaf): coerced
deps (`ThinBoxSlice<TargetLabel>`, `Vec2<ProvidersLabel, …>`),
`CoercedAttr::coerce`, `AttrValues`, the `Arc<HeaderSlice<[CoercedAttr]>>` and
target-label `Arc`s, `TargetNode::new`, and `ArcStrInterner`. These are
long-lived graph nodes allocated amid the transient churn of evaluating each
`BUCK` file — the survivors-pinning-slabs shape. The per-attribute-value slice
of that family (`CoercedAttr` trees, `ArcStrInterner`) has since moved into a
per-package arena (`buck2_node/src/attrs/arena.rs`, which documents the
encoding); the per-node structures (`ThinBoxSlice<TargetLabel>`, deps caches,
`TargetNode` itself) still co-allocate as described below.

Focus mode on the 192 class (its top waste class) sharpened this: only **32% of
its pages are single-leaf cohorts; 68% are mixed**, and the mixtures are exactly
that `TargetNode` family co-residing (`ThinBoxSlice<TargetLabel>` +
`AttrValues` + `CoercedAttr` + `Vec2<ProvidersLabel>`). `ThinBoxSlice<TargetLabel>`
touches 26k pages but is the sole occupant of only 6k — so an arena for any one
of these alone barely frees slabs; the whole co-allocated family has to move
together. (The shift-based size-class table can't see this — it would call those
pages ~98% cohort, the single-sample artifact focus mode exists to avoid.)
