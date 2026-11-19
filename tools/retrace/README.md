# retrace — buck2 build-trace explorer

Timeline GUI and headless analyses for buck2 builds, over the action
trace and real dependency graph captured by the event-log dependency
recorder (`buck2 build --log-action-graph`, `buck2 log graph`).

`--json=cp-rank` ranks conditional action-removal savings (share-of-CP is not
savings: elimination only helps until the shadow path binds), and
`--simulate=NAME|node:N [--scale=F]` replays one action's what-if. An exact name
wins; otherwise a unique substring is accepted. Ambiguous names are refused;
`node:N` selects an action node from the currently loaded graph. The report
prints the new simulated CP and path.
`--simulate-config=free-admission` replays the build with every
execution-admission wait removed. In the GUI, right-click a CP-walker
strip bar or table row to simulate that step at 75/50/25%, removed, or
as a cache hit (lookup only; offered when the step recorded a
cache-lookup time); the walker's Savings column ranks steps by true CP
sensitivity.

`docs/contract.md` holds the honesty rules every emitted number follows,
and `docs/stages.md`
defines the stage counters and names who measured each one.
They may overlap; they are not a partition of wall time.

Action details lead with target, category, start and wall time, followed by
optional **Chronological activity** and the always-visible allocation bar.
**Stages** opens from its disclosure or a click on that bar; chronology is an
independent disclosure above the stage totals. Remote states
and local work share one elapsed-time axis, including recorded cancellations
and overlapping fallback. Execution follows with **Command / stdout / stderr**
tabs, bounded searchable previews and explicit Copy/Save actions. Recorded
command reports remain separate from observed command starts. Missing commands
can be looked up as metadata without downloading their input tree; remote
lookups are always explicit. [Commands and output](docs/execution.md) explains
retention, provenance and limits. **Copy RE diagnostics** copies exact identifiers, observed
intervals and server-log follow-up instructions; it performs no queries
or input downloads. Input-preparation rates use logical bytes and elapsed
preparation time, not measured network bandwidth.

**Inputs by target** in action details groups what the action's direct
dependencies produced by the target that produced it, using the recorded action
graph. A dependency's whole output counts even when the action read only part of
it, and source files, which no action produces, are not listed.

When buck2 runs an action locally and remotely at once, the attempt it cancelled
appears below the action's stages and is not part of them. **Analysis → Cancelled
attempts** lists every cancelled attempt and the stage it was in. A cancelled local
attempt that was still preparing inputs on the build machine can hold up the
remote attempt's input upload while it gathers local files to send. **Upload
breakdown** lists those uploads, and **Ranked findings** replays the critical path
without that wait.

Open **Analysis → Build Timeline** for recorded command lifecycle intervals, Buck's
reported critical-path model and daemon-wide counters as separate evidence.
It opens only when requested, not automatically when loading a capture.
`--json=command-profile` exports the selected run's typed profile with the normal
status/limitations envelope. Older traces without this evidence remain readable
and report the profile as unavailable; action timestamps do not reconstruct it.
Buck model durations are not observed activity intervals. Recorded model offsets
can be aligned approximately to the daemon Command start; the producer does not
record an error bound for that clock alignment. Without that anchor, offsets
remain unaligned rather than receiving invented timestamps.

**Action model CP** is the distinct longest-path calculation over recorded action
dependencies and durations. Its DAG-based what-if, ranking and cascade tools
remain available. The former timing-only Serial Chain, its `--json=serial-chain`
and `--json=levers` exports, and the derived optimal-cores bound have been removed:
nonoverlapping action timestamps do not establish a dependency chain.

Captures also retain artifact dependencies through reused producers, wherever an action
that ran lies below them. These are **structural connectors**, not executions: they
contribute no historical duration, and current execution spans take precedence when
available. Dynamic action aliases resolve through canonical action identities, never
similar display names. This preserves a path such as current A → reused B → current C
without importing an old B execution. Reused producers with nothing that ran below them
are named without their inputs: they are zero-duration leaves, so the critical path and
the cascade are the same as over the whole graph. Missing definitions and incomplete walks
keep the graph partial. Even a verified artifact graph is not a complete scheduling or
whole-command critical-path model.

## Build and test

From the Buck2 workspace:

    cargo build -p retrace
    cargo test -p retrace --lib --test cli_acceptance

[Validation](docs/validation.md) describes the complete gates and the
explicitly opt-in, renderer-specific pixel suite.

## Input

retrace ingests buck2 event logs itself — give it a log file, a
Command ID (the UUID buck2 prints), or `--last` for the newest
local log:

    retrace <trace-id>             # fetch, convert, open the window
    retrace BUILD.pb.zst --json=critical-path
    retrace ingest --last --out=$DIR   # headless: for build pipelines

**Open → Command ID** also accepts queries, targets, audit, test and other
invocations. When no completed actions were recorded, the main window shows
a command profile: identity and outcome, a bounded terminal diagnostic when
available, lifecycle/phase hotspots, Buck's reported path and daemon resource
context. No popup opens automatically. Action timing, action critical path and
simulation are not applicable; zero actions alone do not mean a torn capture.
A no-touch build can still export a structural graph of its reused producers.
`--json=command-profile`, `--json=overview` and `--stats` work for these inputs.
Query-result stdout cannot be reconstructed from timing records. Dedicated
test-result/discovery and BXL-specific views remain follow-up work.

Ingesting writes `<id>-trace.json`, `<id>-execution.bin` (command/output
evidence), and, when available, `<id>-graph.json` under `--out=DIR`.
All are staged before publication, execution and graph before trace;
the renames are not one atomic operation. A validation refusal publishes
none of them. A publication I/O failure may leave a mixed generation,
which the headers' content-generation stamps detect and refuse. Keep the
execution store beside its trace when copying a converted capture.
A build with no usable dependency graph gets a trace and execution store
but no graph (an earlier graph for that build is removed). The viewer forms default to
`~/.cache/retrace` (never the
working directory — `retrace --last` runs from inside a repo); the
explicit `retrace ingest` verb defaults to `.`, like any exporter.
That is the long-term record: buck2 keeps
only the newest few local logs and the uploaded copy has a TTL, while
these artifacts persist for later or batch analysis. A trace id that
is no longer local is fetched from the same store `buck2 log show
--trace-id` reads. The reader dispatches on the dep-capture schema
version stamped in the log and refuses unsupported versions by name. Logs
without dependency capture still provide action or whole-command observations;
they cannot supply an action dependency graph. No legacy converter is bundled.

An already-converted trace works as before:

    retrace build-trace.json       # sibling *-graph.json auto-detected

Opening an event log again reuses its completed conversion: Retrace reads the
invocation header to identify the build, then checks a small
`<id>-ingest-cache.json` receipt before scanning any event records. The receipt
binds the source file's identity/size/timestamps, `--phases`, converter version,
and all artifacts' file identities and matching generation stamps. Status
explicitly says `converting` or `reusing cached conversion`; headless JSON
stdout remains clean. Trace parsing, graph indexing and critical-path analysis
still run on every launch; this is not a persisted in-memory viewer snapshot.

Missing/changed outputs, changed inputs/options and unfinished or undecodable
logs reconvert. Linux and macOS use file identity and change timestamps;
other platforms fingerprint the source and all artifact contents before reuse,
so validation there reads those files even on a cache hit. Hashing is cancellable.
Older outputs without a receipt need one conversion to establish safe automatic
reuse; opening their trace JSON directly still works. `retrace ingest LOG
--out=DIR` always reconverts and refreshes the receipt. These files have no
Retrace TTL or automatic size eviction and survive exiting or interrupting the
viewer. An external cache cleaner may remove them. A per-command writer lock
serializes publication and cache validation across processes. Publication still
uses multiple renames; matching generation stamps detect interrupted writes.

Remote Command ID downloads and remote command-metadata lookup use Meta-only
services and are disabled in open-source builds. Local logs, local Command ID
resolution, recorded command/output evidence, and all offline analyses remain
available. See [integration boundaries](docs/integrations.md).

The observed-activity and command/output schemas change the converter version, so an older
cached conversion is rebuilt once when reopening its event log. An older
trace JSON opened directly stays readable, with limited/missing interval or
execution evidence rather than fabricated data.

A build-ID open checks an existing receipt's unchanged local source before
downloading another log copy. If that source has expired, direct trace-JSON
opening still works independently of Buck's log retention; automatic reuse does
not equate a bare UUID with a particular capture.

Results that depend on an incomplete capture or graph degrade to
`partial`; independent trace-only results do not require a graph.
`--json=health` carries the
build's `outcome`, the capture's state and a `graph` block reconciling
what the header declared with what loaded; `docs/contract.md` has the
rules. A corrupt auto-detected sidecar downgrades to a warning; a corrupt
`--graph=` argument refuses loudly. A sidecar written from a different
artifact generation of the same build is ignored with a warning
(both headers carry a versioned content stamp). An open trace retains
its original file handle across atomic replacement; validation may require
reopening after a replacement or in-place mutation, but never mixes new
bytes with old offsets. A graph with a dependency cycle
is refused; `--repair-cycles` analyses it with the cycle edges dropped,
and everything computed on it is marked partial and is not a bound.

## Loading concurrency

Ingestion parallelizes full protobuf decoding, independent action projection,
and ordered JSON serialization with bounded queues. The viewer can parse its
trace and graph concurrently, then preserves the usual run/pairing checks
before constructing the critical path. Graph-file generation checks guard
against adopting a sidecar that changed during speculative loading. The cache
format and capture-quality rules are unchanged.

Worker counts adapt to available cores. The normal ceilings are four native
decoders, eight trace serializers, four graph serializers/parsers, and sixteen
trace-JSON parsers. These pools do not all run at once. Overlapping trace/graph
loading shares one budget, including its coordinating callers; small CPU
allocations and small inputs use serial/inline paths. `--threads=N` is an upper
bound for trace-JSON parsing, also capped by hardware and the overlapping-load
budget. `0` means automatic and `1` parses inline. It is not a native-ingest or
process-wide thread-limit flag; `retrace ingest` does not accept it.

Queues have byte/record flush targets and fixed live slots, not a hard RSS
limit: the retained graph, allocator overhead and accepted oversized records
are additional. Replacing an already-open view can temporarily retain that
view while a new one loads, and cancellation is cooperative. Retrace does not
change huge-page policy or any system-wide memory setting.

## Sequential critical-path experiments

In **Analysis → Critical Path**, right-click a strip segment or table row and
choose **Cascade…** below **Simulate as cache hit**. The companion window
starts from the current manual overrides and
repeats up to 20 times: find the current critical path, set its longest
positive-duration action to zero, and recompute the path. It chooses from the
new path at every step, not the original top 20 actions or the 20 actions with
the largest standalone savings. A tied alternate path can make a removal save
nothing; the next removal can still help.

The background experiment shows the chosen actions, CP after each removal,
sequential marginal saving, and cumulative saving. It does not change the
manual scenario until **Apply first N removal(s) to overrides** is clicked.
Cancel retains completed rows; changing the scenario invalidates old results.
Removing means zero duration, distinct from **as cache hit (lookup only)**.
These are conditional replays of recorded durations and dependencies with
unlimited resources, not forecasts of actual build wall time. Existing manual
rows show leave-one-out marginal savings; those need not sum, unlike the
cascade's sequential deltas.

Column headers sort the displayed rows; horizontal scrolling keeps every column
accessible in a narrow window. **Step** and **Apply first N** always refer to the
original experiment order, not the current sort. **Entered CP** / **Left CP**
count actions entering/leaving the selected path after that removal, not work
created or deleted; tied paths may produce another representative path.

The Critical Path step arrows and slider reveal and highlight the corresponding
table row, including when the table is sorted.

For reproducible headless analysis starting with no manual overrides:

    retrace build-trace.json --json=cp-cascade --cascade-steps=20 --compact

The default and maximum are 20 steps; `--cascade-steps` accepts 1–20. `--topk` limits
displayed rows, not experiment length. This topic refuses `--simulate`, `--scale`
and `--simulate-config` rather than silently ignoring them. Clear GUI overrides
when comparing it with a GUI cascade.

## Timeline navigation

- **Ctrl+Shift+wheel** zooms time horizontally; **Ctrl+wheel** changes lane height.
  On macOS, **Cmd+wheel** and pinch also zoom time.
- **+ / −** zoom time; **[ / ]** decrease/increase lane height.
- Wheel scrolls lanes; **Shift+wheel** pans time.
- Right-click the bottom overview for zoom buttons and **Reset view**. Reset fits
  the full time range, restores configured lane height and scrolls to the first
  lane, without clearing filters or selection.
- **F** fits time; **Shift+F** fits both time and lanes.

## Settings

Retrace reads an optional TOML file at startup: `$RETRACE_CONFIG` if set,
otherwise `retrace/config.toml` in the platform's config directory
(`~/.config/retrace/` on Linux). It sets colours (`[colors.class]`,
`[colors.stage]`, `[colors.ui]`), view defaults (`[appearance]`,
`[behavior]`) and where the command profile's build link goes:

    [links]
    # `{id}` is replaced with the build ID; "" removes the link.
    build = "https://example.com/builds/{id}"

Without a `build` setting, a Meta build links to the Buck UI and an
open-source build shows no link.

For headless action evidence, use `--json=action-detail --action=EXACT_NAME`
or `--action=action:N` with a selected trace action index. It reads one bounded
source record and exports chronology, coverage, stage-counter provenance and
diagnostic explanations without downloading inputs or loading the graph.
