---
id: event_system
title: Event System
---

Buck2 uses an event stream to connect the daemon, client UI, event logs, and
optional remote reporting backends. The source of truth for event payloads is
`app/buck2_data/data.proto`.

The event system is intentionally broad: the same stream drives terminal
progress, durable event logs, build reports, health checks, and remote build
event uploads. Consumers should prefer typed events over parsing terminal
output.

## Event Model

The core event type is `BuckEvent`. Every Buck event contains:

- a timestamp,
- a trace ID identifying the invocation or background trace,
- an optional span ID,
- an optional parent span ID,
- one event payload.

The trace ID groups events that belong to the same invocation. Span IDs describe
the tree of nested work within that trace.

Buck events are one of three shapes:

- `SpanStartEvent`: starts a span of work, such as command execution, analysis,
  action execution, or test discovery.
- `SpanEndEvent`: closes a span and usually carries duration, stats, and result
  data for the work that finished.
- `InstantEvent`: records a point-in-time event such as a console message,
  warning, action error, target pattern, snapshot, or test result.

There are also client-facing events outside the durable Buck event log:

- `CommandResult`: the final command result returned to the client.
- `PartialResult`: command-specific progress messages.

## Dispatch And Sinks

Most Buck2 code emits events through `EventDispatcher`. The dispatcher attaches
the current trace ID and span context, constructs a `BuckEvent`, and sends it to
an `EventSink`.

Common sink patterns include:

- `channel`: the normal in-process sink used to move events from producers to
  client-side subscribers.
- `tee`: duplicates events into more than one sink.
- `null`: accepts events without recording them.
- `remote`: forwards selected events to a remote backend such as Scribe or BES.

Subscribers consume the stream for different user-visible outputs. For example,
the event log subscriber writes durable invocation logs, while the simpleconsole
and superconsole subscribers render terminal progress.

## Event Logs

The event log is the durable representation of an invocation's Buck events. It
is written in protobuf zstd-compressed form and can be viewed as JSONL with
`buck2 log show`.

See [Logging](./logging.md) for the event log schema details and commands for
viewing recent invocation logs.

## Remote Event Reporting

Buck2 can also forward events to a remote backend. In OSS builds, the BES path
uses the Bazel Build Event Service RPC:

```ini
[bes]
backend = grpc://localhost:1985
results_url = http://localhost:8080/invocation/
```

When a backend is configured, Buck2 serializes Buck events and publishes them
through `PublishBuildToolEventStream`. The invocation trace ID is used as the
BES build invocation ID so that a server can group all uploaded events for the
same command.

### When the stream breaks

A stream whose connection breaks is reopened, and the events the server has
not acknowledged are sent again from the first of them, with the same sequence
numbers, so the server sees one stream. Buck2 keeps a copy of each event until
the server acknowledges it. These `[bes]` keys bound that:

```ini
[bes]
buffer_size = 10000
retry_window_secs = 60
replay_spill_max_bytes = 2147483648
```

- `buffer_size`: the queue between the build and the sink. A stream keeps ten
  times this many copies in memory.
- `retry_window_secs`: how long a stream may keep failing before Buck2 gives
  up on it and drops its events. The default is 60.
- `replay_spill_max_bytes`: past the in-memory copies, the daemon appends the
  oldest ones to a file per stream under `buck-out/<isolation dir>/tmp/bes-replay-spill`
  and reads them back for a replay. The files are deleted when the stream
  closes, is given up, or has every event in the file acknowledged. The key
  bounds the files of all streams together; the default is 2 GiB. Past it, or
  when a file cannot be written or read, the stream lets its oldest copies go,
  and a stream that then breaks cannot be replayed and is given up.

Some servers, BuildBuddy among them, acknowledge nothing until the stream
ends, so a long build relies on the spill to survive a broken connection.

## Bazel Build Event Service Integration

This fork adds an optional BES event format:

```ini
[bes]
event_format = buck
```

Valid values are:

- `buck`: the default. Buck2 sends `buck.data.BuckEvent` payloads in
  `google.devtools.build.v1.BuildEvent.experimental_build_tool_event`.
- `bazel`: Buck2 converts Buck events into Bazel Build Event Protocol payloads
  and sends them in `google.devtools.build.v1.BuildEvent.bazel_event`.

The default `buck` format preserves the existing behavior, but it requires the
BES server to understand Buck2's `BuckEvent` schema. The `bazel` format is a
best-effort compatibility mode for BES servers and web UIs that already know how
to render Bazel BEP, such as BuildBuddy.

Additional Bazel `BuildMetadata` entries can be configured under `[bes]`:

```ini
[bes]
event_format = bazel
build_metadata = PARENT_RUN_ID=workflow-run-id,PARENT_INVOCATION_ID=workflow-invocation-id
```

These entries are merged into Bazel-format `BuildMetadata` events. Metadata
already present on the Buck2 command takes precedence over configured defaults.
`header` and `build_metadata` values support `$VAR` and `${VAR}` environment
variable expansion before Buck2 parses their `KEY=VALUE` entries.

By default the Bazel converter sends an `ActionExecuted` event for every action
it sees. Bazel sends one only for a failed action unless
`--build_event_publish_all_actions` is set, and the same behaviour is available
here:

```ini
[bes]
event_format = bazel
upload_successful_action_events = false
```

With `upload_successful_action_events = false`, a failed action still gets its
`ActionExecuted` event, with its stdout and stderr, and a successful one gets
none. The target, test and output events do not change:
`TargetConfigured`, `TargetComplete`, `TestResult`, `TestSummary` and
`NamedSetOfFiles` still carry every target's outputs, and `TargetComplete`
lists as children only the actions whose events were sent. A large build
sends far fewer events this way, and a BES server that acknowledges a stream
only at its end needs Buck2 to hold far fewer of them for a retry. In the
default `buck` format the key drops successful `ActionExecutionEnd` events from
the stream.

In `bazel` mode, Buck2 translates the event stream into
`build_event_stream.BuildEvent` messages. The converter currently maps:

- command start and finish to `BuildStarted`, structured command line,
  workspace status, build metadata, and `BuildFinished`;
- console output, warnings, structured errors, and action errors to
  `Progress`;
- target pattern, analysis, and build graph information to
  `PatternExpanded`, `TargetConfigured`, and `TargetComplete`;
- action execution to `ActionExecuted`;
- test discovery, test results, and test end events to target, test result, and
  test summary events.

Buck labels are normalized into Bazel-shaped labels for UI compatibility. For
example, root-cell labels such as `root//pkg:target` become `//pkg:target`, and
non-root cells are rendered as external-repository labels such as
`@prelude//pkg:target`.

The conversion is not full Bazel BEP parity. It is intended to make Buck2
invocations useful in Bazel-oriented BES UIs without requiring the server to add
BuckEvent-specific ingestion. Some known limitations are:

- command-line sections are simplified instead of reconstructing Bazel option
  groups;
- Buck output artifacts are not fully modeled as Bazel named sets;
- structured Buck errors are rendered through progress or action stderr when
  there is no lossless Bazel `FailureDetail` mapping;
- target-to-action edges may be emitted as a late `TargetComplete` update after
  action IDs are known;
- Bazel-specific server badges such as CPU, mode, and remote execution status
  may remain unknown until Buck2 emits the fields those UIs expect.

For a local BuildBuddy comparison, see the hello-world example's
`BUILDBUDDY_BAZEL_BES.md`.
