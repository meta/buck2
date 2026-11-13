---
id: perf_regression_hunting
title: Hunting a Regression Between Two Binaries
---

A checklist, not a tutorial. [memory.md](memory.md) covers the per-script
gotchas and sampling math; this page is the workflow on top.

## Setup

- Run in a benchmark-only worktree (see
  [basics.md](basics.md#avoiding-daemon-conflicts)) so you don't fight the
  user's daemon and you don't pay remote-cache splitting from isolation
  dirs.
- Switching binaries kills the daemon (version skew). That's the natural
  way to get fresh DICE between iterations; lean into it rather than
  fighting it.
- For statistical confidence (which you usually need — see
  [benchmarking.md](benchmarking.md)), size the regression with an `abtest`
  run: `--daemon-lifecycle=fresh` records the daemon's peak RSS and
  post-purge jemalloc stats for every sample. The local sequence below is
  for attribution, not detection.

## Single-iteration sequence per binary

[`scripts/measure.sh`](scripts/measure.sh) wraps this. Internally:

1. `buck2 kill`
2. Warmup build with `MALLOC_CONF=prof:true,prof_active:true,prof_final:false`
3. `buck2 kill` (DICE was warm; restart for a clean retained-memory baseline)
4. Measurement build (same MALLOC_CONF)
5. Capture `VmHWM`/`VmRSS` from `/proc/<daemon-pid>/status`
6. `buck2 debug allocator-stats > B_stats.json`
7. `buck2 debug heap-dump --path B.heap`

For peak attribution use [`scripts/peak_watch.sh`](scripts/peak_watch.sh)
instead — the heap dump from step 7 is post-build retained, not peak.

## Diff

```sh
python3 scripts/heap_diff.py /path/to/buck2_a A.heap /path/to/buck2_b B.heap a b
```

Symbol names are stable across unrelated binary changes, so leaves align
even after refactors. To pull the full call chain for a specific leaf:

```sh
python3 scripts/heap_stacks.py /path/to/buck2_b B.heap "<leaf-substring>"
```

Substring matches against both mangled and demangled forms. **Demangled
substrings need to include the angle bracket**: `Foo>::bar`, not
`Foo::bar` (the demangled form is `<…::Foo>::bar`).

## Sanity check

Sum of scaled bytes in the heap profile should be within ~5–10% of
`allocator-stats.allocated`. If it's wildly off, sampling under-counted
small allocations — try `MALLOC_CONF=…,lg_prof_sample:17` for 128 KB
sampling.

## Reading the diff

A leaf with `scale ≈ 1×` is one or a few large allocations; the raw byte
count is the real size and the regression is exactly what it looks like.
A leaf with `scale ≫ 1×` is many small allocations; the regression is
"more of these are alive", and the answer for *why* often lives one or
two frames up the stack rather than at the leaf itself.

The heap-profile leaf labels *where memory was allocated*, not *what's
keeping it alive*. If the diff points at a generic constructor or
`make_*` function with no obvious lifecycle change, the actual regression
is probably one of its callers retaining results longer or more often.
For "what is reachable from X" use the `allocative` path described in
[memory.md](memory.md#when-the-answer-is-in-dice).
