---
id: optimization
title: Observability and Optimization
---

import { FbInternalOnly } from 'docusaurus-plugin-internaldocs-fb/internal';

Optimization involves the use of techniques for determining and improving the
performance of Buck2 and specific actions performed by Buck2. This page covers
the internals for developers of Buck2 and provides details of Starlark that are
likely to be relevant to end users.

## Starlark profiling

`buck2` supports profiling of the evaluation of specific `BUCK` files and
profiling of the analysis of specific targets.

There are three `buck2` profiling commands:

- `buck2 profile loading`
- `buck2 profile analysis`
- `buck2 profile bxl`

For example:

```shell
buck2 profile loading --mode=heap-summary-allocated -o heap-summary.csv //some/package:
buck2 profile analysis --mode=heap-summary-allocated -o heap-summary.csv //some/package:target
```

Possible values for profiling modes are as follows:

- [heap-summary-allocated](#summary-profiling): The heap profile mode provides
  information about the time spent in each function and allocations performed by
  each function. Enabling this mode has the side effect of disabling
  garbage-collection. This profiling mode is the recommended one.
- heap-summary-retained: Like heap summary, but information about retained
  memory after module is frozen.
- time-flame: Provide output compatible with
  [flamegraph.pl](https://github.com/brendangregg/FlameGraph/blob/master/flamegraph.pl).
- heap-flame-allocated: Like heap profile, but writes output comparible with
  [flamegraph.pl](https://github.com/brendangregg/FlameGraph/blob/master/flamegraph.pl).
- heap-flame-retained: Like heap flame, but information about retained memory
  after module is frozen.
- [statement](#statement-profiling): The statement profile mode provides
  information about time spent in each statement.
- bytecode: The bytecode profile mode provides information about bytecode
  instruction pairs.
- bytecode-pairs: The bytecode profile mode provides information about bytecode
  instruction pairs.
- typecheck: Profile runtime typechecking.
- none: Do no profiling.

### Summary profiling

The first profiling mode (`heap-summary-allocated`) provides the time spent
within a function and the allocations that are performed.

As an example, running over a folly BUCK file, produces a CSV file whose
top-left corner is:

```text
Function         Time(s)  TimeRec(s)    Calls   Allocs
TOTALS            10.455      10.455  9712799  3477203
fbchain_configs    1.163       2.514    11328    33984
is_string          0.726       1.028  1514985        0
apple_library      0.725       0.725     1887        0
type               0.435       0.435  2053296        0
...
```

This reveals the following:

- Total execution was 10.455s, which will be a bit slower than normal, because
  profiling is on.
- 1.163s was spent in `fbchain_configs` itself and 2.514s in that function and
  the things it calls.
- A disturbing 1.5M calls and 1.028s is spent testing if things are strings,
  which is almost certainly responsible for half the type calls.
- Happily, `is_string` doesn't allocate, but `fbchain_configs` does. Scrolling
  to the right, on the full CSV file (not shown), reveals it allocates 1 tuple
  and 2 dict per call. It can also be seen that `fbchain_configs` is mostly
  called by `_add_code_coverage_configs`.

This profiling mode is implemented by turning off garbage collection, so the
heap retains everything, and pushing function entry/exit entries on to the heap
with the time they happen. After execution, the heap can be scanned in order to
reconstruct the call tree and allocation patterns. As a result, this profile
mode may consume significantly more memory.

### Statement profiling

The second profiling mode tells us which statements spent most time executing.
Running it over a structured-logger `BUCK` file gives us a CSV file starting
with:

```text
File                            Span  Duration(s)    Count
TOTAL                                        4.03  7187761
fbcode_allowed_list.bzl  420:9-423:1         0.27   455884
cell_defs.bzl             13:5-13:60         0.17   117736
read_configs.bzl          46:5-46:55         0.08    65042
prelude.bzl               28:9-29:20         0.07     1004
...
```

This profile shows how much time is spent in each statement. Looking at the
relevant portion of `fbode_allowed_list.bzl`:

```python
for _package in _recursive_allowlist:
    if base_path == _package or base_path.startswith(_package + "/"):
        return True
```

The `if` statement is at location 420:9-423:1 and takes 0.27s. The `if`
statement runs approximately 456K times. While looking at the outer statement in
the profile (not shown), it can be seen that the `for` loop is only called 3188
times, implying an average of 143 iterations per call. It's possible that this
loop could be rewritten as some clever dictionary lookup, perhaps iterating over
the path components of `_package`.

Line profiling builds on top of the `before_stmt` hook that is used for
debugging. It records the time each statement is entered then blames that
statement for all time until the next statement. That means that sometimes, due
to statements making function calls, the `return` of the function call may be
'blamed' until the next statement executes. As a result, treat the results with
slight caution.

### Flame profiling

The flame profiling modes produces a `.svg` flamegraph showing either time spent
or allocations. You can open it in Google chrome and inspect the resulting flame
graph.

<FbInternalOnly>

The flame profile provides a list of how time is used based on call stacks (you
can download an example [here](https://www.internalfb.com/intern/px/p/1Mz2W)).

</FbInternalOnly>

## Native profiling

- Profiling on Linux can be done with
  `perf record -g --call-graph=dwarf,20000 ...` and `perf report --call-graph`
  - Don't profile the `buck2` process directly unless you are interested in
    profiling the CLI; you likely want to profile the `buck2` daemon process.
    You can find the pid with `buck2 status` and attach `perf` to that PID.
- Profiling on Mac can be done with `Instruments`<FbInternalOnly> (for details,
  see the Wiki article
  [Running and Testing Builds](https://www.internalfb.com/intern/wiki/GraphQL/Build_Infra/Running_and_Testing_Builds/#profiling-the-rust-code))</FbInternalOnly>.

## Benchmarking

- If you want to do proper statistically relevant A/B testing, use
  `absh -a testa -b testb` (see [absh](https://github.com/stepancheg/absh) in
  the GitHub repository).
- To measure the number of instructions:
  - On Linux, use `perf stat foo`
  - On Mac, use `/usr/bin/time -lp foo`
- On Mac, to run something with the time profiler on the command line, use
  `xcrun xctrace record --template 'Time Profiler' --launch -- foo`, then
  `open Foo.trace` for the name of the trace file it spits out (or pass
  `--output` to control the output filename).
