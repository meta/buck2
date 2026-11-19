---
id: logging
title: Logging
---

import { FbInternalOnly } from 'docusaurus-plugin-internaldocs-fb/internal';

Buck2 produces detailed event logs for each invocation, which follow a schema
outlined in `app/buck2_data/data.proto` in the buck2 parent directory. The event
logs that Buck2 produces automatically are always in protobuf zstd-compressed
format (see [Viewing the event log](#viewing-the-event-log) for more details).

## Event log format

Warning: the schemas are all subject to change, so we do not recommend relying
on the format. For the source of truth, take a look at `data.proto`.

### Invocation header

The first line of the event log is always the `Invocation` header:

```python
Invocation {
    # CLI args split into a list of strings
    command_line_args: List[str],
    # Expanded CLI args, which expand any argsfiles
    expanded_command_line_args: List[str],
    # Absolute path of the current working directory of the Buck2 command
    working_dir: str,
    # UUID of the Buck2 command
    trace_id: str,
}
```

### Command result footer

The last line is always the `CommandResult`:

```python
Result {
    # One of the result types of CommandResult protobuf type in daemon.proto.
    # CommandResult has 23 result variants in total, including BuildResponse,
    # TestResponse, CqueryResponse, UqueryResponse, AqueryResponse,
    # TargetsResponse, TargetsShowOutputsResponse, BxlResponse, InstallResponse,
    # ProfileResponse, LspResponse, DapResponse, AllocativeResponse,
    # CleanStaleResponse, SubscriptionCommandResponse, TraceIoResponse,
    # ConfiguredTargetsResponse, GenericResponse, KillResponse, StatusResponse,
    # PingResponse, and ErrorReport.
    result: BuildResponse | CqueryResponse | BxlResponse | ...,
}
```

### Buck events

The rest of the event log contain `BuckEvent`s, which are either
`SpanStartEvent`s, `SpanEndEvent`s, or `InstantEvent`s.

The `BuckEvent` format is roughly as follows:

```python
Event {
    # When the event was fired. This is always a 2-item list, where the first
    # value is seconds since the Unix epoch, second value is nanoseconds
    timestamp: List[u64],
    # UUID of the Buck2 command, same one as the invocation header
    trace_id: str,
    # A trace-unique 64-bit integer identifying this event's span ID,
    # if this event begins a new span or belongs to one.
    span_id: u64,
    # A trace-unique 64-bit identifying the span that this event is logically
    # parented to.
    parent_id: u64,
    # See sections below for more details
    data: SpanStart | SpanEnd | Instant,
}
```

#### Span starts

The `SpanStartEvent` indicates that a span of work starting:

```python
SpanStart {
    # One of the data types of SpanStartEvent protobuf type in data.proto
    data: AnalysisStart | ActionExecutionStart | ...,
}
```

#### Span ends

The `SpanEndEvent` indicates that a span of work has finished:

```python
SpanEnd {
    # Duration of the span
    duration_us: u64,
    # CPU poll times for this span
    stats: SpanStats,
    # One of the data types of SpanEndEvent protobuf type in data.proto
    data: AnalysisEnd | ActionExecutionEnd | ...,
}

# CPU poll times for this span
SpanStats {
  max_poll_time_us: u64,
  total_poll_time_us: u64,
}
```

#### Instant events

The `InstantEvent` represents a single point in time:

```python
InstantEvent {
    # One of the data types of InstantEvent protobuf type in data.proto
    data: ConsoleMessage | ActionError | ...,
}
```

One specific instant event type that may be of interest is the `Snapshot` event,
which includes some interesting details like RSS, CPU, I/O, remote execution,
and DICE metrics.

## Viewing the event log

Event logs can be accessed using commands under `buck2 log show`, which outputs
the event logs in JSONL format. You can run `buck2 log show --help` to see all
available options. Some useful commands:

- Show the logs for the most recent Buck2 command:

```sh
buck2 log show
```

- Show the logs for a specific Buck2 command, given the command's UUID:

```sh
buck2 log show --trace-id <UUID>
```

- Show the logs for a recent Buck2 command:

```sh
buck2 log show --recent <NUMBER>
```

<FbInternalOnly>

You can also download the logs locally from Buck2 UI. The logs will be
downloaded from Manifold in protobuf zstd-compressed format, and you can view
them in JSONL format by passing the path into `buck2 log show`.
</FbInternalOnly>

The JSON schema is derived from the protobuf types, and the log itself could be
quite large. [jq](https://jqlang.github.io/jq/) can be useful to find specific
things. For example, this jq script shows the max event delay between a snapshot
event creation on the daemon side, and when the client receives it.

```sh
buck2 log show | jq -s '
  map(
    .Event.data.Instant.data.Snapshot.this_event_client_delay_ms
      | select(. != null)
  ) | max'
```
