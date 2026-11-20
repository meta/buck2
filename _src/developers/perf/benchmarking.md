---
id: perf_benchmarking
title: Benchmarking
---

[basics.md](basics.md) covers the perf basics shared with profiling. This
page is benchmarking-specific: variance, sample sizes, fair comparison
between two versions of Buck or of the repo.

## Effect sizes

We typically aim to detect changes down to 0.3–0.5%; anything over 1% is a large win or regression.
Single-shot measurements detect ~nothing in this range — every benchmark needs many samples.

## abtest

[`scripts/abtest`](../../../scripts/abtest/) is the standard A/B benchmarking tool (Meta-only).
Given a revision per row, it schedules a Skycastle workflow that builds Buck, runs the benchmark
command many times per row striped across many hosts, and reports each metric as a per-row ratio
with a confidence interval computed across hosts.

```sh
# CLI recipes for pairing revisions and rerunning rows; read before scheduling
buck run @fbcode//mode/opt fbcode//buck2/scripts/abtest:abtest -- agent-instructions

# Compare two Buck revisions on the current repo state
buck run @fbcode//mode/opt fbcode//buck2/scripts/abtest:abtest -- \
  abtest --remote -b DBASE -b DCHANGED -r . -d base -d changed \
  -- audit providers --quiet @fbcode//mode/opt fbcode//buck2/app/buck2:buck2-bin

# Block until the workflow finishes, then write graphs + stats locally
buck run @fbcode//mode/opt fbcode//buck2/scripts/abtest:abtest -- \
  analyze --workflow-run-id <id-or-url> -o /tmp/ab.png --text_output /tmp/ab.txt
```

- `-b` is the revision Buck is built at; `-r` is the repo checkout the command runs in (prelude,
  macros, configs, workload). Both accept hashes, D-numbers, or `.`; `-b buck2` runs the released
  Buck instead of building one.
- Everything after the second `--` is the Buck command to benchmark; the workload advice in
  [basics.md](basics.md#workloads) applies.
- `--remote` gives each shard its own machine. The `--local` default runs the whole workflow on
  your machine and is only good for smoke-testing a setup, along with `--dummy` and `--no-action`.
- `--platform macos` or `--platform windows` runs the command on that platform's Sandcastle
  workers (Linux is the default). Buck is still built on Linux, cross-compiled (`opt-mac-arm64`,
  `opt-win`); only the benchmark shards move, so pick a workload and mode that evaluate there,
  e.g. `audit providers --quiet @fbcode//mode/opt-mac-arm64 <target>`. `--unicastle-size` is
  Linux-only.
- Scheduling prints the workflow URL; `analyze` waits for the workflow, so it can be run
  immediately after.
- Start with the default `--samples 100 --shards 50`. All shards run at once and each runs its
  share of every row back to back, so a run takes about `rows × (samples / shards + 1 warm-up)`
  samples' worth of time. Raise `--shards` when the command is slow. Intervals are computed across
  shards, so if the reported interval is wider than the effect you're hunting, more shards help
  there too.

## Metrics and `--daemon-lifecycle`

`--daemon-lifecycle` decides how buckd is managed around each sample, and thereby which metric
answers which question:

| Question                                 | Lifecycle                           | Metric column      |
|------------------------------------------|-------------------------------------|--------------------|
| "How long does buck2 take?"              | any (`none` has the least variance) | Wall Time          |
| "How much memory at peak?"               | `fresh`                             | buckd Max RSS      |
| "Peak of a single `--no-buckd` process?" | `none`                              | Process Max RSS    |
| "How much does the daemon retain?"       | `fresh`                             | jemalloc Allocated |
| "Does the daemon grow across commands?"  | `reuse`                             | jemalloc Allocated |
| "Is fragmentation to blame?"             | `fresh` / `reuse`                   | jemalloc Waste     |
| "How much CPU did the daemon spend?"     | `fresh`                             | buckd CPU Time     |

- `fresh` (the default) kills buckd around every sample; `reuse` keeps one daemon per host alive
  across samples; `none` adds `--no-buckd`.
- In `fresh`/`reuse`, the memory columns come from `buck2 status --snapshot` after purging the
  allocator, so "jemalloc Allocated" is retained memory and "buckd Max RSS" is the daemon's real
  peak. The process columns in those modes describe the thin gRPC client
  ([basics.md](basics.md#the-process-model)) — ignore "Process Max RSS" there.
- With `reuse`, "buckd Max RSS" is the peak since the daemon started, not per sample.
- On Windows there is no jemalloc, so only the RSS, CPU and wall-time columns exist, and "buckd
  Max RSS" is the peak working set. The raw per-process fields (`ru_*` from `wait4` on Linux and
  macOS, `process_*` on Windows) are in `samples.json`.

## Reading the output

The workflow (and `analyze`) produce a histogram grid (`buckabtest.png`) and text stats
(`buckabtest_stats.txt`), plus `*_filtered` variants with IQR outliers dropped. For each metric,
every row after the first is summarized as `B/A <ratio> <low>..<high> => <pct>%`: the paired
ratio against row A with its 95% interval.

- Take the warnings in the text output seriously — dropped shards, shards sharing a physical
  machine, and mixed CPU models all mean the interval is less trustworthy than it looks.
- If the filtered and unfiltered results disagree materially, look at the histograms before
  believing either.

## Per-iteration variance

| Metric                     | Stddev across runs                |
|----------------------------|-----------------------------------|
| Wall time                  | 100 ms – 1 s on a 15 s build      |
| Daemon `VmHWM` (peak RSS)  | ~50 MB on a 4–5 GB build          |
| jemalloc `allocated`       | a few MB; very stable             |

`allocated` is stable enough that small samples are usable; for peak RSS and wall time, sub-1%
effects only emerge from pairing many samples across many hosts, which is what abtest does.

## Quick local checks

For coarse local iteration — effects of several percent, or checking that a workload behaves
before scheduling — [absh](https://github.com/stepancheg/absh) is convenient:

```sh
buck2 build @fbcode//mode/opt fbsource//third-party/rust:absh-absh --out /tmp/absh
# -i: ignore the first iteration; -r: randomize A/B order; -m: max RSS of the spawned process
/tmp/absh -a '/tmp/b2a ...' -b '/tmp/b2b ...' -i -r -m -n 30
```

`-m` is only meaningful with `--no-buckd`, where the spawned process is the one doing the work.
Run local loops in a benchmark-only worktree
([basics.md](basics.md#avoiding-daemon-conflicts)); switching binaries between iterations kills
the daemon via version skew, which conveniently gives fresh DICE each time.
