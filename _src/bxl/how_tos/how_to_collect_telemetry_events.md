---
id: how_to_collect_telemetry_events
title: How to Collect Telemetry Events
---

## Telemetry

### Emitting events from your BXL script

In BXL, you can emit custom events via `ctx.instant_event()`, which takes in two
named parameters:

- `id`: string, identifies your event. Helpful to identify your event when
  looking through event logs. Ids do not have to be unique in a single BXL
  script.
- `metadata`: dict, where keys are strings, and values are strings, bools, ints,
  or lists/dicts of the mentioned types. You can put any metadata you wish here.

Example:

```python
def _impl(ctx):
  ctx.instant_event(id = "id1", metadata = {"foo": "bar"})

my_script = bxl_main(
  impl = _impl,
  cli_args = {},
)
```

Only instant events can be manually created within BXL at this time, which means
that the event represents a single point in time. If you need something similar
to spans (start and end events which encompass a range of time) for measuring
the duration of a particular section (excluding actions - see below for more
information), you could couple instant events with the global `now()` function
to measure the duration yourself:

```python
def _impl(ctx):
  instant = now()

  # do something time intensive
  end = instant.elapsed_millis()
  ctx.instant_event(id = "id1", metadata = {"duration": end})

  # do something else time intensive
  end = instant.elapsed_millis()
  ctx.instant_event(id = "id2", metadata = {"duration": end})

my_script = bxl_main(
  impl = _impl,
  cli_args = {},
)
```

**Measuring time for actions and ensuring artifacts**

You cannot use `now()` to measure the time it takes to run actions and ensure
artifacts because these processes occur asynchronously outside of the BXL script
execution. For BXL user telemetry, we emit action events via the buck2 core
automatically. Events around ensuring the artifacts are not emitted currently,
but will be added soon.

### User event log

To write to your own event log when running BXL, you can run your BXL command
with the `--user-event-log` flag to tell buck2 where to write the events to.
Buck2 is aware of the following file extensions: `.json-lines`,
`json-lines.zst`, `.json-lines.gz`, and will compress the files automatically
for you depending on the extension. If the extension is not one of these, the
logs will always be written in JSONL format, uncompressed.

Example:

```sh
buck2 bxl path//to/my_script/script.bxl:my_script --user-event-log my_file.json-lines.gz
```

When using this flag to write to a custom event log, it is up to you to clean up
these log files. In addition, if the same filename is used with subsequent BXL
invocations, events are always appended to the existing file contents, which is
the same behavior as `buck2 <any command> --event-log <path>`. If you tell buck2
to write to a compressed file, you are responsible for decompressing them.

### Getting a user event log from a normal event log

`buck2 log show-user` can be used to convert a normal event log (regardless of
encoding/compression) to a user event. Similar to `buck2 log show`, you can
choose the most recent invocation, or the nth invocation, or provide a path to
the normal user event log. Note that user event logs are not able to be passed
into `buck2 log show` or `buck2 log show-user`.

### Event log output

The first line of your event log will always be the invocation record, which
contains useful things like command line args used, working directory, etc. The
subsequent lines are either instant events and/or action events, depending on
your BXL script's contents.

**Instant event**

Sample:

```python
{
  "StarlarkUserEvent": {
    "id": "foo",
    "metadata": {
      "bool_value": true,
      "string_value": "str",
      "int_value": 123,
      "list_value": [
        "a",
        "b",
        "c"
      ],
      "dict_value": {
        "foo": "bar"
      }
    },
  },
  "epoch_millis": 123456789 # when the event was emitted
}
```

**Action event**

```python
{
  "ActionExecutionEvent": {
    "kind": "Write", # kind of action, like write or run
    "name": { # name of the action, for user display. Unique within the execution of a particular target
      "category": "write", # category for the action
      "identifier": "my_output" # identifier for the action
    },
    "duration_millis": 0, # duration of the action in millis, excluding input materialization time
    "output_size": 10, # size in bytes of the action's outputs
    "input_materialization_duration_millis": 0, # how long it took to materialize any inputs to the action
    "execution_kind": "Simple", # how the action was executed
    "owner": "cell//path/to/script.bxl:function_name" # owner of the action execution (target label, anon target label, bxl label)
  },
  "epoch_millis": 123456789 # when the event was emitted
}
```

`execution_kind` includes:

- Local: action was executed locally
- Remote: action was executed via a remote executor
- ActionCache: action was served by the action cache and not executed
- Simple: action is simple and executed inline within buck2 (ex: write,
  symlink_dir)
- Skipped: action was not executed at all
- Deferred: action logically executed, but didn't do all the work
- LocalDepFile: action was served by the local dep file cache and not executed.
- LocalWorker: action was executed via a local worker
- NotSet: action execution kind was not set

**Ensure artifact event**

```python
{
  "BxlEnsureArtifactsEvent": {
    "duration_millis": 0, # duration of ensuring the artifact
  },
  "epoch_millis": 123456789 # when the event was emitted
}
```
