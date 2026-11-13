---
id: perf_memory
title: Memory Profiling
---

[basics.md](basics.md) covers the headline metrics, the daemon model, and the
`debug allocator-stats` / `debug heap-dump` / `kill` commands. This page is
the additional, mostly buck2-specific things that the heap-profiling tool
chain will trip you up on.

This page is about *where allocations happen* (the heap profile). For the
`active - allocated` **slab-fragmentation** gap — partially-empty slabs pinned
by survivors, which a heap profile cannot see — see
[memory_fragmentation.md](memory_fragmentation.md).

## Enable profiling

The daemon must be **started** with profiling on:

```sh
MALLOC_CONF=prof:true,prof_active:true,prof_final:false buck2 <cmd>
```

`prof_final:false` is the important non-default — it suppresses the auto-dump
on exit, which we don't want. Without `MALLOC_CONF=prof:true` set in the env
when the daemon was started, `debug heap-dump` returns an error and you must
restart the daemon.

`--no-buckd` mode runs everything in one process that exits before you can
ask for a heap dump. If you must heap-profile a `--no-buckd` run, set
`MALLOC_CONF=…,prof_final:true` to dump at exit — but that captures
post-cleanup state, not peak.

## Allocator stats vs RSS

`allocator-stats.allocated` is what to track for retained memory: it counts
bytes the program has malloc'd and not freed. The relationship to `VmRSS`:

- `allocated` ≤ `active` ≤ `resident` ≈ `VmRSS`. The slack between
  `allocated` and `resident` is jemalloc page slack — pages backing freed
  allocations that haven't been returned to the OS yet.
- After a build, jemalloc's bg threads spend ~10–30 s decommitting unused
  pages. RSS visibly drops in that window. **`allocated` does not change**
  during decommit, because no user code freed anything. Always check
  `allocated`, not RSS, to tell whether something actually freed.
- `retained` and `metadata` from `allocator-stats` are jemalloc-internal
  bookkeeping and almost never the answer to a question.

## Peak vs retained

A `debug heap-dump` is a snapshot at the moment you call it — typically
after the build, when transient allocations (in-flight tokio task records,
starlark eval bumpalo heaps, scratch hashmaps) have been freed. The gap
between `VmHWM` and post-build `allocated` is dominated by these
transients, not by retained data.

To attribute peak, dump *during* the build when RSS is at a new
high-water mark: [`scripts/peak_watch.sh`](scripts/peak_watch.sh) polls
`/proc/<daemon-pid>/status` and triggers `debug heap-dump` on each new HWM,
keeping only the latest. Don't reach for `MALLOC_CONF=…,prof_gdump:true`
to do this — at the default sample rate it fires constantly and produces
tens of thousands of files, exhausting FDs and crashing the daemon. (If
you really want gdump, raise `lg_prof_sample` to ≥24.)

## Sampling

jemalloc samples each allocation with probability `1 - exp(-size/period)`,
`period = 2^lg_prof_sample` (default 19 = 512 KB). The non-obvious
consequence:

- One huge allocation (multi-MB hashtable, GB-scale Vec) is sampled at
  ≈100% — the raw byte count in the profile equals its true size.
- Many small allocations are heavily under-counted in raw bytes, with the
  underestimate proportional to `period / avg_size` for that site.

The correct adjustment is **per-stack**, not uniform. For each stack,
multiply by `1 / (1 - exp(-avg/period))` where `avg = bytes/count`. A
plausible-looking "scale every site by `allocated / raw_total`" is wrong:
it badly inflates large-allocation sites and is the most common
misinterpretation of a raw heap profile.
[`scripts/heap_aggregate.py`](scripts/heap_aggregate.py) applies the
per-stack scaling.

## Don't use jeprof

`jeprof` shells out to `addr2line`. On a full-debug `buck2` binary that's
many minutes per profile because the DWARF section is huge and the binary
contains references to missing `.dwo` files. `eu-addr2line` is no faster.

[`scripts/heap_aggregate.py`](scripts/heap_aggregate.py) symbolicates with
`nm` + binary search and runs in seconds. For diffing two profiles or
extracting a specific stack see the other scripts in that directory.

## Demangling Rust v0 names

`c++filt` does not handle Rust v0 (`_R…`) names, even with `--format=rust`.
Use `rustfilt`. Inside fbsource it lives at
`fbsource/third-party/rust/tools/rustfilt`. The included scripts auto-detect
it.

`.llvm.NNN` suffixes (LLVM-added for inlined functions) sometimes cause
rustfilt to leave a frame undemangled even though it can demangle the
same name in isolation. When this happens it's a frame-by-frame artifact;
re-pipe the offending symbol on its own to demangle it.

## Reading a heap profile

- The deepest frame in every stack is jemalloc's own backtrace function
  (`prof_backtrace_impl` / `jemalloc_je_*`). The actual user allocation
  site is the first non-jemalloc frame walking up.
  `heap_aggregate.py` skips jemalloc frames automatically.
- The leaf is the *allocation site*, not the *retainer*. A `Box::new` in
  function `make_foo` may move into a struct that's stored on a DICE node
  somewhere else; the heap profile points at `make_foo` either way. If
  the question is "what's keeping this alive", the heap profile is at
  best a starting point.

## When the answer is in DICE

For "what is reachable from object X" rather than "where was X allocated",
use [`allocative`](https://github.com/facebookincubator/allocative) (already
integrated into buck2). The DICE state implements `Allocative`;
`buck2 debug allocative` serializes a graph for offline analysis. This is
the right tool when a heap profile shows lots of attribution to upstream
constructors but you actually want to know which DICE keys are retaining
the data.
