# The honesty contract

Every number retrace emits — a `--json` topic, a `--stats` line, a GUI
panel — travels inside a light envelope that says what the number is
worth. Presenters may summarize the envelope; they may not drop it.
This is the working rule that kept two independent implementations in
microsecond agreement across six corpora: when a tool cannot compute
something honestly, it says so instead of printing a plausible value.

Adapted (lightened) from traceviewer-v2's `AnalysisResult`/`Capability`
contract; credit to that design for the status/coverage/limitations
separation and the no-stored-percentages rule.

## The envelope

```json
{
  "status": "available" | "partial" | "refused",
  "bound":  "lower" | "upper",          // optional, see below
  "coverage": [
    {"scope": "actions-with-graph-nodes", "numerator": 294971, "denominator": 294982},
    {"scope": "graph-rows-declared", "numerator": 1425673, "denominator": 1425673}
  ],
  "limitations": ["11 actions have no graph node (dynamic outputs?)"]
}
```

- **status** — `available`: the full input supported the computation.
  `partial`: the value is real but some of the input was unusable; every
  gap is counted in `coverage` and named in `limitations`. `refused`:
  no value at all, `limitations` says why. A capture known to be
  incomplete — no footer, a nonfinal footer, footer counts that
  disagree with what was recorded, or a log that stopped decoding —
  makes every topic at best `partial`, including topics that never
  read the graph. A capture whose
  completeness was never recorded, or that was never enabled, is not a
  gap. One decision (`quality.rs`) makes every envelope, the `--stats`
  text and the GUI chips agree.
- **bound** — present only when the labelled quantity is provably a
  bound, not an estimate: a critical path computed while some trace
  rows lack graph nodes is a *lower* bound and is labelled as one. A
  saving obtained by replaying that same partial graph is not a bound
  in either direction — a missing edge can raise or lower it — so
  what-if, cp-rank and findings envelopes never carry `bound`; on a
  partial graph they say the saving is conditional on the captured
  graph. `lower` is claimed only when every graph gap is an omission
  (a missing row, edge or line): contradictory/surplus counts and a
  repaired cycle are not, so neither justifies a lower bound. A graph whose pairing with
  the trace could not be checked keeps the bound and says so. A bound
  is not a degraded estimate; it is the strongest true statement
  available.
- **coverage** — counted numerator/denominator pairs per named scope.
  Percentages are never stored; presenters derive them, so a rounded
  "100%" can never mask a nonzero gap.
- **limitations** — human-readable, machine-carried strings. The GUI
  renders them as compact health chips with drill-in — never inline
  text walls, never silently dropped.

## Graph scope is separate from capture integrity

`--log-action-graph` records schema 1. It records every action that ran with its direct
inputs, and after the command it defines the reused actions and transitive sets those
inputs refer to. A reused action or set with an action that ran somewhere below it gets
a full definition, with its own inputs; the others the recording refers to are named
without their inputs (`inputs_omitted`, `contents_omitted`), because everything under
them took no time in that command. A command where no action ran records no definitions.

Retrace reads schema 1 only; any other declared schema fails closed, by name. Start and
footer must agree. A capture where actions ran and no reused producer was defined is the
command's whole graph (scope `full`). Otherwise it is scoped to what the actions that ran
depend on (`current_action_ancestors`), which includes a command where nothing ran.
Completeness follows `buck2 log graph`: the footer is present and does not mark the graph
incomplete, its action-end, set-definition, structure and alias counts match the stream,
and nothing referenced is undefined.

Retrace full sidecars retain `kind:"full", enc:2`. Activity sidecars use
`kind:"current_action_ancestors", enc:3` and preserve scope in `dep_capture` metadata.
Older readers reject `enc:3`; new readers reject inconsistent kind/encoding/scope.
An intact scoped capture can have zero nodes on a no-touch command.

Current-action critical-path and replay results remain available with their normal
bounds when the scoped capture is intact. They carry a scope note and
`graph_scope:"current_action_ancestors"`; intentionally omitted non-current graph
regions are not an integrity failure. Whole-build descendant/fanout reports are
scope-limited and marked partial. Missing or contradictory records still degrade
claims under the normal rules. No historical duration is assigned to a structural
node. Optional `structure_wall_time_us` measures the producer's successful
structural walk, not action time or client log draining, and never enters the DAG.

## Refusals (loud, never silent)

- A dependency graph without a supported encoding stamp (`enc:2` full,
  `enc:3` current-action ancestors): refused. Analyzing a flattened graph as if it
  were the real one produces confident nonsense.
- A trace whose graph references transitive-set join rows that no row
  defines, when the log reports no dropped lines: the capture is
  corrupt or was written by a mismatched producer — refused. (With a
  torn tail on record, the gaps are counted as dangling instead and the
  result degrades to `partial`; a killed build is not corruption.)
- An ambiguous `--run` selector over an appended multi-run trace:
  refused, listing the candidates. Guessing the run would silently
  analyze the wrong build.
- A dependency graph that contains a cycle: refused. Deleting rows from
  a real build graph cannot create a cycle, so a cycle means the file
  is corrupt or two different actions share one name.
  `--repair-cycles` drops the edges that close each cycle instead;
  every result built on that graph is then `partial`, carries no
  `bound`, counts the kept edges in `coverage`, and says it was
  computed on a repaired graph.

## Ingest outputs

### Commands without completed actions

Validated command metadata does not require action rows. Run selection uses
invocation identity, including for queries, no-op builds and failures before
execution. Overview, health and command-profile remain available; action-only
topics return a refused envelope explaining that they are not applicable.
No synthetic actions are invented. An explicitly activity-scoped graph may be
empty; otherwise a graph requires recorded action or structural evidence.
Absence of actions is not itself
evidence of capture truncation; existing source and profile limits still apply.

Native ingestion uses recognized terminal-response contracts for outcomes.
The omitted deprecated proto3 `CommandEnd.is_success` scalar is not proof of
failure. Missing or unrecognized results remain unknown unless another reliable
signal establishes an outcome. A retained terminal diagnostic is bounded to
16 KiB and is not query stdout or a reconstructed test report. Converter cache
versioning prevents reuse of older output with different outcome semantics.

An ingest stages three artifacts beside their final names: the execution
evidence store, dependency graph when available, and trace. After validation,
publication renames them in that order: execution, graph, trace. A validation
refusal leaves existing final artifacts unchanged and removes its staging files.
When this conversion produces no graph, the earlier `-graph.json` for that
command is removed after publishing the trace; status output records the removal.
After successful publication, eligible completed conversions record a separate
cache receipt. Failure to write that receipt may require reconversion next time;
it does not invalidate the published evidence.

A per-command writer lock serializes conversions of the same command in one
output directory. A reader that starts after ingest returns sees matching
artifacts. Readers already open during publication are not coordinated with the
writer, and the renames are not one atomic transaction. A mixed trace/graph
generation is caught by the
`capture` stamp both headers carry, derived from the complete emitted
trace and graph content under a versioned hashing contract, including
footer quality, graph definitions and encoder version. The execution-store reader
also verifies run, capture stamp and `StoreInfo` against the trace before exposing
recorded commands or output streams. Publication I/O failure can leave a mixed
generation between renames; identity checks detect it, but publication does not
roll back already renamed artifacts. A trace
and a graph whose stamps differ, or of which only one is stamped, are
refused as a pair: a discovered sidecar is ignored with a warning, an
explicit `--graph=` refuses. Two unstamped files, written before the
stamp existed, still load. A graph header naming a different run than
the selected trace run is refused the same way.

The parsed trace retains its source file handle. Later extras/detail
reads use that same generation and independent read positions, not a
fresh open of a mutable path. Atomic replacement therefore leaves an
existing reader on its original file; conservative metadata validation can
require reopening even after replacement. Detected in-place changes refuse
subsequent reads. Producer completeness does not excuse dropped trace
rows or unfinished run records: artifact health is checked independently.

## Exit codes

- `0` — a result was emitted. A failed *build* in the capture is data
  (it appears in `Result`/`failed` fields), not a tool failure. A build
  whose outcome the log does not record is reported as unknown, never
  as success.
- `2` — refusal or usage error, with the reason on stderr: unreadable
  or over-cap input, an unsupported-encoding, cyclic or corrupt graph, an
  ambiguous or unknown `--run`, an unknown `--json` topic. An ingest
  validation refusal publishes no new trace, graph or execution store; existing
  final artifacts remain intact. Publication I/O failures have the narrower
  guarantees described above.

## Accounting rules

- The command profile and the action dependency model are separate surfaces.
  `command-profile` exports the selected run's recorded lifecycle intervals,
  Buck critical-path entries and daemon counter samples without combining their
  different clocks or scopes. Native traces carry this as `ph:"M"`,
  `cat:"command-profile"`, `name:"command_profile"`, with
  `args:{run,profile}`; `profile.schema_version` is currently `1`. The run ID
  binds the profile to its selected invocation. A reported model duration is
  not an observed interval. Recorded model offsets may use an approximate
  alignment to the daemon Command start, marked `approximate_alignment`; no
  alignment error bound is recorded. Without that anchor, model offsets remain
  unaligned. A duration cannot supply a missing timestamp. Missing profiles are
  explicitly refused with null data, never synthesized from the
  action timeline. Rejected/dropped evidence and producer qualifications remain
  visible in the typed profile and its limitations.
- **Action model CP** uses recorded action dependencies and durations, not the
  temporal proximity of actions. Serial Chain and its network Levers estimates
  are no longer interfaces; the action-DAG what-if scenarios remain conditional
  model calculations. Time outside the first-to-last action window is reported
  without assigning an unrecorded cause such as startup, loading or analysis.
- Dangling references (a dep whose producer is absent from the log, a
  join token no definition names) are *counted*, reported in coverage,
  and gate the result to `partial` — they are never both dropped and
  uncounted.
- Graph lines that are not JSON are counted. A torn tail (the last
  lines of a killed build) is told apart from malformed lines inside
  the file, and only a torn tail relaxes the undefined-reference
  refusal. A line that is JSON but of the wrong shape is a bad row,
  counted separately, and a rejected line leaves no name behind that a
  trace action could match.
- Denominators come from the input's own declarations (the graph
  header's coverage block, the trace's row count), not from what the
  loader happened to keep. Declared-versus-observed disagreements —
  header nodes and edges against rows loaded, footer counts against
  records recorded — are counted, reported in coverage and gate the
  result to `partial`. An absent declaration is reported as unknown,
  never assumed satisfied.
- Coverage scopes are separate for actions with a defining graph row
  (`actions-with-graph-nodes`), for declared rows and edges, and for
  the capture's declared counts. An action whose name appears only as
  another row's dependency is not covered: its defining row is
  missing.
- Zero-weight structural rows (transitive-set join nodes) never enter
  work/time denominators; they carry connectivity, not cost.

## Build outcome and capture completeness are independent

`run_end.outcome` (`success`, `failed` or `unknown`; `exit` is null
when unknown) records what the log says about the build. A log with
neither the client's command result nor the command end (a build that
was killed, a torn capture, a build still running) is unknown, and
every reader of the outcome, the overview row included, says so
rather than defaulting to success.

`run_end.dep_capture` and the graph header's `coverage.dep_capture`
carry the same object and record whether the dependency capture is
whole: whether it was enabled at all (`not-enabled` is not a gap),
whether the footer is present and explicitly final, its declared counts
against what the reader recorded, and whether the log decoded to its
end. The two are independent: a failed build can have a complete
capture, and a successful build an incomplete one. A capture that falls
short warns in `health` with each shortfall named; an absent declaration
is reported as unknown, never taken as satisfied.

## Topic naming

`--json` topic names are hyphenated on the command line (`cp-rank`,
`re-queue`, `critical-path`); the emitted JSON object keys are the
same names with underscores (`cp_rank`, `re_queue`, `critical_path`).
One name, two spellings, mechanical mapping — tooling that joins on
keys should expect the underscored form.

## The execution-admission band

Rung-2 instrumented captures carry, per remote action in `args.re`:
`submit_us` (the submit interval: from asking for an
execution slot until RE accepts the stream), `rs_exec_sem_wait_us`
(parked on buck2's execution_concurrency_limit — the dominant
component), `rs_rpc_open_us`, `rs_request_prep_us`, and
`rs_exec_sem_permits_available` (slots free when the action queued).
The submit interval covers the permit wait, then request preparation,
then opening the stream. The unexplained remainder subtracts permit
wait, RPC open, and preparation. The submit total mixes
waiting and work; only `rs_exec_sem_wait_us` measures execution-slot
queueing. These are parallel-lane times — do not read their
totals as wall. An absent measurement is not a measured zero. The
`free-admission` config what-if (`--simulate-config=free-admission`,
and the CP walker's toggle) subtracts only valid recorded
`rs_exec_sem_wait_us`, preserves recorded current execution, and replays.
The stream open and request preparation remain; the result is a
conditional model change, not a capacity-change prediction.

## Stacked what-if overrides

Per-action overrides in the CP walker are applied on top of the
whole-graph free-admission transform and win for the nodes they edit.
A row's savings cell is its marginal: the simulated critical path with
only that row reverted to the base the rest of the scenario stands on
(the admission-free weight when that toggle is on, the measured one
otherwise), minus the scenario's critical path. Marginals are signed. Zero means
reverting this row alone does not lengthen the scenario's critical path; it is not
a statement about path membership. Marginals do not add up because changes can
expose shared or alternative critical paths.

The cache-hit replay replaces a step's duration with its own recorded
`stages.cache_query_us`. It is lookup only (the restore a hit would
also pay is not captured by native ingest) and it is refused when the
record has no such key; a recorded zero is a measurement and is
offered.

## Transfer pricing

Bandwidth what-ifs are hypothetical bytes/rate replacements, not measured
network bandwidth or predictions of a faster link. Input preparation uses
logical input bytes, including files that may be cache-resident. Output upload
uses logical output bytes; neither size is measured traffic.

The client input-upload leg uses the recorded batch size
(`args.re.iu_bytes_uploaded`, nominal, before retries), or declared input size
(`args.bytes_in`) as an explicit proxy when that size was not recorded. When
the CAS-call duration was recorded, only that part of the upload interval is
replaced; otherwise the whole upload band is modeled. That replacement does
not prove all CAS-call time was network service. A measured zero call duration
or zero-byte batch leaves this leg unchanged. Invalid call durations or batch
sizes disable the leg rather than selecting a fallback. Other valid legs
remain usable, and the replay preserves recorded current execution.

Each scenario reports measurement coverage and its assumptions. Missing bytes,
partial timing, and overlapping counters limit the interpretation; elapsed-time
residuals are not silently assigned to storage.

Retrace does not infer daemon backlog or pipeline service rates. Recorded
residuals and positioned gaps do not prove post-worker queueing or a
service-rate ceiling.

## Retry fates

`--json=retries` classifies every discarded attempt by FATE against
the attempt that won: `concurrent` (its interval overlaps the winner;
the causal delay effect is not established), `rerun` (ended before the
winner started) or `unknown` (the
capture gives no interval retrace can compare). There is no
infrastructure-vs-user split: an earlier `infra_churn` count
classified by status STRING, and every status ("Cancelled" included)
turned out to carry both fates.

How a fate was decided travels beside it as `fate_basis`. The
interval relation is exact (`client-interval`) when both the discarded
attempt and the winner carry `client_start` and positive matching command
durations, within the containing action: a buck2 whose command
records include `client_attempt.start_time` writes each attempt's start on the
buck2 host's clock, and that start with the attempt's wall time is one
interval on one clock. The winner's duration is `args.winner.dur_us`,
not the action's total `wall_time_us`. Without comparable starts, an attempt is `concurrent` only
when its duration and the winner's cannot both fit inside the action's
span (`duration-overflow`); anything else is `unknown`. A missing
winner duration or a zero-length attempt (a record buck2 wrote without
measuring the attempt) is `unknown`, never a zero-length winner placed
at the action's end. The exact rule assumes the host's clock did not
step between the two attempts; retrace does not detect steps. Captures
made before the field existed show mostly `concurrent` and `unknown`.

`lost_us` is the discarded attempts' command elapsed time as buck2
measured each, which can include client queueing and download; it is
not worker occupancy. `unknown` attempts count in `lost_us` and never
in `delay_us`. The census carries the coverage pairs
`discarded-attempts-with-known-fate` and
`discarded-attempts-with-exact-interval`, and is `partial` when any
attempt is unclassified or count-only. `delay_us` is the rerun share's
summed pre-winner elapsed budget, not measured critical-path impact or
a bound on contention from concurrent/unknown attempts.

A retry finding with classified attempts and a usable graph reports
`estimate: "cp-replay"`: the captured critical path minus the replayed
critical path after subtracting each action's classified pre-winner
budget from its duration, floored at zero, with other costs and edges
fixed. This is a conditional scenario, not measured savings or a causal
bound on the benefit of eliminating retries. Unknown and count-only
attempts are outside that scenario; when no attempts are classified,
or no graph replay is available, `opportunity_us` is absent and
`estimate` is `"none"`. A concurrent-only scenario can report zero
because it removes no pre-winner budget; that does not establish zero
contention or zero build impact.
