# The stage taxonomy

An action's wall time is one number and it explains nothing. The stage
taxonomy puts named counters beside it, identifying recorded
work or waiting and who counted each duration. These counters can
overlap or leave unexplained time; they are not an exact partition or
a chronological reconstruction of the action's wall time.

The rule the taxonomy follows is the same one `docs/contract.md` states
for every emitted number: **a reader looking at a duration must never
have to guess where it came from.** That is why provenance is the first
line of every stage hover and not a footnote, why the residual has its
own band instead of being spread across the others, and why there is no
band called "upload".

## The stage bands

Stable counter order, not chronological event order. The key
is the `args.stages` field name; `STAGE_ORDER` in `src/detail.rs` is the
single array everything else is parallel to.

| # | key | label | measured by |
|---|-----|-------|-------------|
| 0 | `cache_query_us` | cache lookup | client |
| 1 | `input_upload_us` | inputs upload (to CAS) | client |
| 2 | `exec_slot_wait_us` | waiting for an execution slot (client-side limit) | client |
| 3 | `queue_us` | RE queue | client |
| 4 | `materialize_us` | worker input preparation (CAS) | worker\* |
| 5 | `hashing_us` | hashing | sum of recorded durations (may overlap) |
| 6 | `execute_us` | execution | worker |
| 7 | `upload_us` | outputs upload (worker to CAS) | worker, or observed client progress state* |
| 8 | `result_wait_us` | result stage (legacy) | ambiguous legacy counter* |
| 9 | `download_us` | download / materialize (from CAS) | client |
| 10 | `rtt_us` | round trips + retries | derived legacy residual |
| 11 | `daemon_us` | uncovered time (client-span residual) | **derived** |
| 12 | `cache_hit_us` | cache-hit result handling | client |
| 13 | `prepare_us` | action request preparation | client |
| 14 | `cas_artifact_us` | CAS artifact preparation | client |
| 15 | `local_finalize_outputs_us` | local output finalization | client |
| 16 | `local_cache_check_us` | local cache check | client |

\* band 4 flips to "measured by this machine's materializer" when the
action ran locally: a local action's input fetch is this machine writing
to disk, not a worker filling a sandbox. Execution explicitly marked
`execute_source = "client_span"` also uses client provenance rather
than the generic worker-counter provenance. Record-specific stage lines
carry that source through both their hover and copied text.

Current `upload_us` records worker output preparation/upload. Its
`upload_source` identifies a current worker duration (`worker`) or the
client's observed worker-upload progress state (`client_state`). The
latter includes notification delays and is not network service time.
Older native conversions placed that progress state in `result_wait_us`;
the legacy counter alone cannot distinguish worker upload from other
result handling. It does not prove client queueing or idle time. Reopen
the original event log to recover the current separation. `rtt_us` is
also retained for older captures; stage families are not assumed disjoint.

`materialize_source = "client_span"` identifies local input preparation
observed by the client, distinct from worker-reported input preparation.

`cas_artifact_us` covers recorded client phases for CAS-backed artifact rules:
expiration checks, optional TTL extension and recheck, tree metadata preparation,
and output declaration. Their clipped union forms one counter band; chronological
activity retains the individual phases. Tree metadata preparation includes fetching
and decoding metadata and preparing its directory representation, not downloading
artifact contents. These client intervals do not identify backend-only latency.

`local_finalize_outputs_us` covers collecting, hashing, preparing metadata, and
declaring local command outputs after execution. The separate `hashing_us` counter
sums output-file hash durations, which can overlap one another and this parent.
When the final command's timing explicitly identifies `hashing_source = "local_outputs"`
and a valid finalization counter is present, hashing appears as a subordinate detail,
not another additive band. The raw hash sum is retained even when larger than its
parent. A recorded zero parent is valid; a missing or invalid parent does not suppress
the hash counter. Legacy or other sources retain the generic **hashing** label.

These phases require the corresponding producer instrumentation. Older captures do
not gain invented zeros or intervals; a new conversion cannot recover unrecorded spans.

`execute_us` paints in the action's build-type colour. It measures command
elapsed time, not necessarily productive CPU time. Other counters describe
surrounding work or waiting, not necessarily idleness.

## Compact action details: totals and optional chronology

The header shows **Target**, **Category**, **Started**, and **Wall time**,
followed by a divider. Wall time stays next to the timing section rather
than among the execution metadata. The optional **Chronological activity**
section starts collapsed, above the always-visible **Stage totals** allocation
bar. **Stages** starts collapsed below the bar: clicking the bar opens it,
and clicking a segment highlights the corresponding stage. The disclosure
arrow toggles it independently; another bar click opens rather than closes
it. The disclosures reset when a new action detail opens. **Execution**
follows, then **Graph & context** when available.

Expanding **Chronological activity** shows recorded client-clock intervals
on a relative-time axis, plus observations and gaps sorted by start time.
Client operations, remote progress states and local execution occupy
separate rows; simultaneous observations in the same group get separate
rows too. Rows are not invented worker attempts. An end event is not proof
of success. Hatching marks a recorded cancellation, not CPU inactivity or
its cause. Worker duration counters are never given made-up positions on
this axis.

Relative offsets in a version-1 record do not require an absolute action
timestamp to be displayed. Legacy observations require enough recorded
timestamps to position them; duration-only legacy counters cannot supply
the missing chronology. Missing or invalid evidence is identified explicitly,
not replaced with fabricated intervals. Usable limited observations remain
distinct from a complete account of the recorded interval list.

New native conversions preserve `args.activity` version 1:

```
{"version":1,"spans":[["Re.WorkerDownload",3139358,579439598,true]]}
```

Each tuple is `[name, offset_us, duration_us, cancelled]`, with the offset
relative to this action's timestamp. All intervals come from the client's
recorded stage starts and ends; unfinished starts are not closed at EOF.
The optional `rejected` count exposes invalid producer records. Consumers
validate and clip intervals to the action before taking their **union**.
Only that union's complement is a gap without a recorded state. Overlap
does not count twice. Invalid/incomplete interval data cannot establish a
complete complement; old captures remain usable with explicit limited
evidence. An empty new span list is different from an absent old field.

The **Stage totals** bar and **Stages** list summarize duration counters,
not event order. Their counters mix measurement sources and can overlap;
their positive `wall - counters` remainder is not an extra observed wait.
A remote input-preparation state may cover almost the entire action,
including local fallback, even when the final successful command has only
a few seconds of execution counters. Expanding the chronology exposes that
overlap without turning the counter remainder into hundreds of seconds of
daemon CPU.

**Input preparation is not network throughput.** Logical input bytes include
cache-resident files; preparation includes tree traversal, local cache and
filesystem work, shared-download waits and possible retries. The displayed
effective rate is logical input size divided by preparation elapsed time.
The build-wide comparison is contextual, not a controlled comparison of
workers, cache warmth, action kinds or network performance.

**Copy RE diagnostics** copies build ID, exact action/configuration/digest,
span ID and RE session when recorded, the action time window and observed
intervals. It does not query services, fetch input trees, or rerun actions.
Use those identifiers to join retained worker execution records to CAS
`download_directory` requests. CAS phase timers can distinguish directory
setup, blob materialization and shared-download completion/retry handling.
To attribute a delay to a storage backend, follow a concrete blob/request
or trace ID into backend latency/error logs. Backend byte/query counts alone
do not establish backend latency. Missing backend records remain unknown.

### Cached execution is history, not current work

A cache hit can carry the original command's execution duration. It
must not enter this build's execution counter. The analysis and GUI
readers suppress unmarked `execute_us` on cache hits, including older
already-converted traces. Native ingestion preserves known reused
command timing as `args.historical_execution.exec_us`, with
`args.execution_metadata = "reused"`, rather than a current-execution
alias.

A cache result can nevertheless follow a real local attempt. Native
ingestion retains the clipped union of captured `Local.Execute` and
`Local.WorkerExecute` spans as `stages.execute_us`, explicitly marked
`stages.execute_source = "client_span"`. Readers retain that observed
current activity even when the final result was cached. These are
client-side elapsed spans, not worker CPU time or a new retry model.
Lookup and materialization counters remain current costs.

`cache_hit_us` is current client activity accepting cached outputs: fetching and
decoding output-tree metadata when needed, constructing output metadata, claiming
outputs, and declaring them to the materializer. Declaration can be lazy; this is
not necessarily downloading output-file bytes and is never historical execution.
`prepare_us` constructs and serializes the action request before lookup/execution;
it is not all prerequisite work, all input hashing, or remote submit preparation.
Both counters use unions of their recorded client intervals, clipped to the action,
including handled cancellation. They do not change the action's duration or CP.

Older converted JSON can contain these chronological spans without named counters.
Reopen the original event log to recover the classification; no new build is
required. Existing JSON counters are not silently reconstructed from potentially
incomplete intervals. Simulated cache-hit replacement deliberately remains the
recorded lookup cost alone; it does not add preparation or cached-result handling.

Not every historical producer identifies remote result reuse. An
unmarked remote command is not classified as reused merely because
worker CPU or duration fields are absent; those captures retain their
existing interpretation. Full attribution of hidden RE-side reuse
requires affirmative producer provenance.

### Local cache validation is real client work

`local_cache_check_us` is the clipped union of local `MatchDepFiles` spans.
Chronology distinguishes identical-action checks from filtered-input checks.
Remote dep-file validation remains a separate observation; it is not added to
the local band or counted twice as another remote lookup.

These checks can await output-presence validation with the materializer. An
**in-memory hit** describes the source of the cached entry, not the latency of
the whole check. It does not mean the action ran again or that its outputs were
fetched from the network.

Optional `args.re.local_cache` details retain counts of observed lookup outcomes:
miss, in-memory hit, persisted hit, error, unreported, or unknown future enum.
They count checks rather than distinct actions. Persisted-store probe/fetch
timings and fetched-candidate counts are subordinate measurements, not extra
bands or a guaranteed partition. Missing timers do not become zero, and a
canceled check has no invented outcome.

Existing event logs already contain these spans. Re-converting them can recover
their previously discarded coverage without repeating the build.

### Why there is no band called "upload"

Inputs move before the action runs, from this machine, through the
client's CAS path. Outputs move after it runs, from the worker, through
a different path — and the client receives only a receipt, an exit code
and output digests, not bytes. Calling both "upload" is not a
simplification; it is a category error, and it hid a queueing pathology
for as long as it stood. The same discipline is why `download_us` says
"download / materialize": the client pulls output bytes lazily, only
when something local needs them.

## Measurement provenance

`StageProv` describes the measurement source; record-specific markers can
override a band's generic source:

- **worker** — counted on the remote worker and returned in its
  response, for example worker input preparation and execution.
- **client** — a span this machine's buck2 daemon opened and closed.
  For example cache lookup, admission or output materialization.
- **local materializer** — this machine's materializer, for band 4 on a
  locally-executed action.
- **remote state** — the client's elapsed observation of worker progress,
  including notification delay; not a worker service-time measurement.
- **duration sum** — summed recorded spans, which may overlap, such as hashing.
- **legacy** — an older counter whose measurement source is not disambiguated.
- **derived** — a residual rather than a directly measured operation.

The distinction is not decoration. A client-side span measures *when the
daemon noticed*, which on a loaded daemon can be much later than when
the thing happened; a worker counter measures the thing. Text surfaces
that have nowhere to hang a hover — the copied popup, the analysis
prompt — get `stages_prov_line`, which folds the whole table onto one
line naming only the bands actually present. A number without its source
beside it is the failure mode the table exists to prevent.

## The residual is three different things, kept apart

Each action is reconciled separately before its accounting is added to
a path using `CpAccounting::add_step`. Clamping after summing a path
would let excess counters in one action cancel unexplained time in
another:

- **`unaccounted`** = the sum of each measured action's positive
  `wall - staged` residual.
  Unattributed elapsed time: the action and bands were recorded, but
  no band claims this residual. It does not establish idleness.
- **`unmeasured`** = wall belonging to steps that carry no stage counters
  at all, with `n_unmeasured` saying how many steps those were. This is
  the *absence of a measurement*, not a measured gap. The GUI hatches it
  for exactly that reason.
- **`overlap`** = the sum of each measured action's positive
  `staged - wall` excess. It can reflect overlapping or inconsistent
  counters; it is not an exact measurement of temporal overlap.
  Nonzero excess means the counters cannot be read as a partition.

For example, two 10-second actions with 15 and 1 seconds of counters
have **9 seconds unaccounted and 5 seconds excess**, not 4 seconds
unaccounted and zero excess. The CP footer reports both. Its bar is
explicitly a counter composition, using wall plus excess as its
denominator when necessary, not a timeline or additive wall-time split.

Band 11 (the retained wire key `daemon_us`) is a fourth thing again:
the derived client-span residual *within* an action's own accounting.
The UI calls it **uncovered time (client-span residual)**. Neither the
key nor the duration proves daemon CPU work, bookkeeping, executor
queueing, or idleness. Gaps reflect the converter's observed coverage;
missing or previously discarded intervals can enlarge them.

Every surface that summarizes the same window — the CP walker's verdict
line, its stage bar, table footers — reads from the same per-action accounting. One
reconciliation, so they cannot disagree with each other.

## Sub-rows: the bands that decompose

Some bands carry their own decomposition, rendered inline where the
whole is rather than in a section two screens below. Sub-rows are empty
on captures that predate the fields, and the parent row alone is then
the whole story — an absent sub-row never means zero.

**Inputs upload** (`args.re.iu_*`) records four phase durations: waiting
for an upload slot, checking what storage has, gathering local files
(`iu_materialize_us`, which includes asking the materializer where each
file is, also shown on its own as `iu_materializer_query_us`), and the CAS
write call. Below them, one line gives the bytes sent (`iu_bytes_uploaded`)
and how many of the checked digests storage lacked (`iu_digests_absent` of
`iu_digests_queried`). When the action's cancelled local attempt (see
**Hybrid races**) was preparing the same inputs while files were being
gathered, a line under the gathering phase gives how long the two overlapped;
the overlap is an observation, not a measured cause. These do not necessarily
cover the entire parent interval. The permit wait is admission waiting; the CAS call can include
coordination and other work as well as transfer. An unexplained remainder
does not establish storage delay.

A measured zero CAS duration contributes no duration to replace in the
transfer scenario; it does not by itself prove why the call was zero.
A recorded zero-byte batch likewise provides no transfer bytes to price,
even if the call took time. The upload batch size (`iu_bytes_uploaded`)
is nominal, before retries. Digest classifications are not counts of files
physically written. Invalid counter fields remain unavailable while valid
sub-fields, including recorded zeros, are retained.

When a global pass is available, the popup can show other modeled upload
bands active at admission. These windows assume upload follows cache lookup
and include both holders and waiters: this is modeled occupancy, not queue
depth.

**Hybrid races**: buck2 can run an action locally and remotely at once and
keep whichever finishes first. The attempt it cancelled is listed after the
stages, with the stage it was in and for how long, from the cancelled
intervals in `args.activity`. Its time ran alongside the stages and is not
added to them. A cancelled local attempt that was still preparing inputs
leaves those downloads running in the background.

**Worker input preparation** shows the logical size of the inputs
(`bytes_in`) and, when RE reported it, how much the worker actually downloaded
(`args.re.worker_bytes_in`); the rest was already cached on the worker.

**Execution-slot wait** (`args.re.rs_*`) is shown inside its submit
window, alongside the unexplained remainder and how many execution
slots were free when the action asked. The window covers the permit
wait, then preparing the request, then opening the connection.
Preparation is elapsed work, not CPU time, and is subtracted from the
submit remainder. The total submit span is not all queueing; only the
measured semaphore wait is.
The band itself is synthetic: it is sourced from
`args.re.rs_exec_sem_wait_us`, not from `args.stages`, so it is zero —
and therefore invisible — on uninstrumented captures, which render
exactly as they did before the band existed.

**Client-span residual** (`args.re.gap_*`) cannot name its causes, but it
can say where it sits: before the first observed client span, between
observed client spans, or after the last observed client span. Those
positions do not by themselves identify preparation, executor waits, or
result processing. For `run` actions the recorder also measures preparing
the command, before the cache check (`args.re.prepare_command_us`), and
finishing the action, after the result arrived (`args.re.finish_action_us`); the popup shows how
much of the first and last gaps those account for, and the rest stays
unnamed. Worker counters and hashing duration sums can overlap these gaps.

**Local commands** carry the working directory when it is not the project
root (`args.cwd`; tests run in their cell's root) and the rule buck2 used to
build the command's environment (`args.env_rule`), which the popup shows next
to the command line.

**Cache lookup** (`args.re.cache_query`) separates `permit_wait_us` from `sdk_us`.
The former is waiting for Buck's action-cache permit; the latter is the admitted
client/SDK operation, including its own queues, retries and response handling—not
network RTT or backend-only latency.

**Cache-hit result handling** (`args.re.cache_hit`) can report output-tree
admission (`tree_admission_us`), fetch and typed decode (`tree_fetch_us`), output
metadata construction and ownership claim (`local_bookkeeping_us`), and materializer
declaration (`declare_us`), alongside `tree_count`. Missing phases, including paths
that bypass an operation, are not fabricated as zero.

These nested fields are inclusive client phase observations inside the parent
stage, not extra additive bands or a guaranteed partition. `records` counts observed
stage records and `measured` counts records carrying the new diagnostics; the UI
identifies partial coverage. Completed phase values survive handled failures.
`field_records` identifies sums covering only some measured records; invalid or
overflowed fields listed in `invalid_fields` stay unavailable. No child durations
are inferred by subtracting from the parent or attributed to a particular backend.

### Remote state observations are not additional bands

The detail popup and copied analysis prompt keep these optional `args.re`
fields in a separate **RE observations** row:

- `worker_download_wait_us`: the clipped union of observed
  `Re.WorkerDownload` client-state intervals, including canceled ones.
- `worker_download_cancelled_wait_us`: the clipped union of the subset
  ending with a recorded `SpanCancelled`, not a second independent cost.
- `reported_first_enqueue_to_worker_us`: the legacy reported interval
  from first enqueue to the final worker, which may include worker
  activity and retries. It is not a measurement of queue wait alone.
- `execution_attempts` and `final_queue_us`: when RE attempted the action
  more than once, how many times, and how long the final attempt queued.
  The difference from the interval above is time spent on earlier attempts.

Worker-download state can overlap local fallback work. It does not
measure physical network service, attribute transferred bytes, or add
an exclusive stage-counter budget. Including a canceled interval in
observed coverage does not say why it was canceled; unfinished starts
are not given fabricated end timestamps. Missing observation fields
remain absent, while an explicitly recorded zero remains zero.

New conversions prefer the recorded `RemoteCommand.queue_time`
(`queue_time_us` in event-log JSON), then the observed `Re.Queue`
interval union, for `queue_us`. They keep
the wider reported scheduling interval separately instead of calling
it queueing. Already-converted older traces retain their numeric
counters; their RE queue hover warns that those counters may include
the wider interval.

## What gets highlighted, and what never does

`slow_stage_marks` decides which bands pop, under deliberately narrow
rules:

- nothing highlights on an action under 2 s of wall;
- candidates are non-exec measured bands taking at least `max(25% of wall, 1s)`;
  derived residuals and ambiguous legacy counters never qualify;
- the single largest candidate paints red, any other candidate bolds —
  two red "% of wall" claims side by side read as an action that is more
  than 100% itself, so exactly one thing pops;
- if nothing qualifies but *recorded* non-exec time is at least half the
  wall, the largest non-exec band paints red.

That last clause is recorded stage time, not wall-minus-exec, and the
difference matters: unaccounted time is nobody's wait, and blaming a
tiny band for it painted noise red.

`execute_us` never highlights under these rules. The highlight draws attention
to surrounding elapsed costs; it does not prove any highlighted cost was idle.

## Capture dependence

The taxonomy degrades by band, never by refusing the trace:

| band or sub-row | needs |
|---|---|
| bands 0, 3–9, 11–13 | the corresponding recorded action evidence; graph capture is not required |
| bands 14–15 | recorded CAS-artifact and local-finalization client spans |
| band 16 and local-cache details | local `MatchDepFiles` spans and optional end diagnostics |
| local output hashing sub-row | explicit local-output source and valid finalization counter |
| band 10 | the retained legacy counter |
| band 2 and its sub-rows | `rs_*` instrumentation |
| inputs-upload sub-rows | `iu_*` instrumentation |
| cache lookup/result sub-rows | optional `cache_query` / `cache_hit` diagnostics |
| client-span residual sub-rows | `gap_*` fields (converter rung 1) |
| RE state observations | corresponding optional `args.re` fields |

A missing band has no numeric claim and does not paint. Recorded zeros remain
measurements for coverage and accounting; an absent or rejected field must not
be interpreted as a measured absence of waiting.

## Cache lookup includes the client's entire observed span

Band 0 reflects Buck's `CacheQuery` span
(`app/buck2_data/data.proto`, `ExecutorStageStart.cache_query`). It can include
client admission, SDK queues and response-processing costs. Similar completion times
or high outstanding counts can motivate investigation, but do not identify a
shared barrier or quantify queue time. Retrace does not invent a queue/lookup
split from timestamp clustering; only the explicitly recorded optional phase
measurements supply that split.
