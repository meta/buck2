---
id: persistent_workers
title: Persistent Workers
---

import { FbInternalOnly, OssOnly } from
'docusaurus-plugin-internaldocs-fb/internal';

Persistent workers are long-lived processes that Buck2 spawns once and reuses
across multiple build actions, avoiding the overhead of repeatedly starting
heavy processes like compilers. This is particularly useful for JVM-based tools
where startup cost is significant. Workers are shared across actions within a
single build command and are terminated when the command completes, unless
[`buck2.persistent_workers_across_commands`](#keeping-workers-across-commands)
keeps them for later commands.

## How worker identity works

Each call to [`WorkerInfo(...)`](../../api/build/WorkerInfo) in Starlark is assigned a unique internal ID.
Buck2 uses this ID to decide whether to reuse an existing worker process or
spawn a new one: actions that reference the **same `WorkerInfo` instance** share
the same worker process, while actions referencing **different instances** get
separate workers.

This means that where you construct `WorkerInfo` matters:

- **Creating `WorkerInfo` in a separate rule** (and referencing it via a dep or
  toolchain) ensures all actions that use that dep share one worker process.
- **Creating `WorkerInfo` inline** in a rule implementation means each target
  gets its own worker, because each analysis call produces a new `WorkerInfo`
  with a new ID.

## Defining a worker

A worker is defined by a rule that returns a [`WorkerInfo`](../../api/build/WorkerInfo) provider:

```python
def _worker_impl(ctx: AnalysisContext) -> list[Provider]:
    return [
        DefaultInfo(),
        WorkerInfo(
            exe = ctx.attrs.exe[RunInfo],
            concurrency = ctx.attrs.concurrency,
        ),
    ]

worker = rule(
    impl = _worker_impl,
    attrs = {
        "exe": attrs.exec_dep(providers = [RunInfo]),
        "concurrency": attrs.option(attrs.int(), default = None),
    },
)
```

`WorkerInfo` accepts the following parameters:

- **`exe`**: The command to start the worker process. Typically a `RunInfo` or
  `cmd_args`.
- **`concurrency`**: Optional maximum number of concurrent commands the worker
  can handle. When `None`, Buck2 sends one command at a time.

Instantiate this in your `BUCK` file:

```python
worker(
    name = "my_compiler_worker",
    exe = "//tools:my_compiler",
    concurrency = 4,
)
```

## Using a worker in an action

To run an action through a worker, pass a [`WorkerRunInfo`](../../api/build/WorkerRunInfo)
as the `exe` parameter to `ctx.actions.run()`. `WorkerRunInfo` bundles a worker
with a fallback command for when workers are disabled:

```python
def _my_binary_impl(ctx: AnalysisContext) -> list[Provider]:
    output = ctx.actions.declare_output(ctx.label.name)

    # The positional arguments are what gets sent to the worker process.
    args = cmd_args(ctx.attrs.source, output.as_output())

    ctx.actions.run(
        args,
        category = "compile",
        exe = WorkerRunInfo(
            # Which worker process to use (identified by WorkerInfo instance)
            worker = ctx.attrs._worker[WorkerInfo],
            # Fallback executable when workers are disabled.
            # Uses .args to extract the cmd_args from a RunInfo.
            # Do NOT include action-specific arguments here — the positional
            # arguments above are appended automatically in the fallback path.
            exe = ctx.attrs._compiler[RunInfo].args,
        ),
    )
    return [DefaultInfo(default_outputs = [output])]

my_binary = rule(
    impl = _my_binary_impl,
    attrs = {
        "source": attrs.source(),
        "_compiler": attrs.exec_dep(
            default = "//tools:my_compiler",
            providers = [RunInfo],
        ),
        "_worker": attrs.exec_dep(
            default = "//:my_compiler_worker",
            providers = [WorkerInfo],
        ),
    },
)
```

When `exe` is set to a `WorkerRunInfo`:

- **With workers enabled**: The worker process (identified by
  `WorkerRunInfo.worker`) receives only the positional `arguments`.
  `WorkerRunInfo.exe` is unused.
- **Without workers (fallback)**: The final command is `WorkerRunInfo.exe`
  followed by the positional `arguments`. So `WorkerRunInfo.exe` should only
  contain the executable and its fixed flags — do not include the
  action-specific arguments, as they are appended automatically.

## Sharing workers via toolchains

The recommended way to share a single worker across many targets is through a
[toolchain](writing_toolchains.md). Define the worker as a target and reference
it from the toolchain:

```python
MyToolchainInfo = provider(fields = {
    "compiler": provider_field(RunInfo),
    "worker": provider_field(WorkerInfo),
})

def _my_toolchain_impl(ctx: AnalysisContext) -> list[Provider]:
    return [
        DefaultInfo(),
        MyToolchainInfo(
            compiler = ctx.attrs.compiler[RunInfo],
            worker = ctx.attrs.worker[WorkerInfo],
        ),
    ]

my_toolchain = rule(
    impl = _my_toolchain_impl,
    is_toolchain_rule = True,
    attrs = {
        "compiler": attrs.exec_dep(providers = [RunInfo]),
        "worker": attrs.exec_dep(providers = [WorkerInfo]),
    },
)
```

Because both `compiler` and `worker` reference the same target instance across
all consumers of the toolchain, all actions using this toolchain share one
worker process.

:::warning

Do **not** create `WorkerInfo` inline in a rule implementation if you intend to
share a worker across targets. Each call to `WorkerInfo(...)` produces a new
instance with a new ID, resulting in a separate worker process per target.

:::

## Protocols

Buck2 supports two persistent worker protocols:

- **Local workers** use a Buck2-specific gRPC protocol over Unix domain sockets.
  Buck2 passes the socket path in the `WORKER_SOCKET` environment variable.
- **Remote workers** use the
  [Bazel persistent worker protocol](https://bazel.build/remote/persistent)
  (length-prefixed protobuf over stdin/stdout). Enable this with
  `supports_bazel_remote_persistent_worker_protocol = True` on `WorkerInfo`.

For remote workers, command-line arguments must be passed via an argument file
(`@argfile`).

## Configuration

<FbInternalOnly>

Workers can be enabled or disabled at the execution platform level via the
`use_persistent_workers` attribute on `CommandExecutorConfig`, or globally via
the `build.use_persistent_workers` buckconfig (defaults to `True`).

</FbInternalOnly>

<OssOnly>

Workers are enabled at the execution platform level via the
`use_persistent_workers` attribute on `CommandExecutorConfig`, which defaults to
`False`. The execution platform that `buck2 init` configures,
`prelude//platforms:default`, sets it from the `build.use_persistent_workers`
buckconfig, which defaults to `True`, so an action whose `WorkerRunInfo` has a
`worker` runs in that worker. An execution platform that you define must set
`use_persistent_workers = True` itself, as `platforms/BUCK` does in
[examples/persistent_worker](https://github.com/meta/buck2/tree/main/examples/persistent_worker);
on one that leaves it unset, every action runs the fallback `exe` of its
`WorkerRunInfo`. A project that configures no execution platforms at all, such
as one made with `buck2 init --no-prelude`, runs its actions with workers
enabled.

</OssOnly>

### Keeping workers across commands

With `buck2.persistent_workers_across_commands = true`, a local worker outlives
the command that started it and serves later commands of the same daemon, so
they skip its startup and warm-up:

```ini
[buck2]
persistent_workers_across_commands = true
# Seconds a worker waits for its next action before it stops; the default is 900.
persistent_worker_idle_timeout_s = 900
```

Across commands, a worker is identified by its command line, environment,
`concurrency` and `streaming`, not by its `WorkerInfo` instance, so a target
that is analyzed again keeps its worker. A worker whose input files changed
since it started, for example a rebuilt tool jar, is replaced. So is a worker
that failed to start, exited, or was still running a command when a cancellation
or a timeout made Buck2 stop waiting for it: the next command starts a new one.

An idle worker keeps its memory until it stops.
