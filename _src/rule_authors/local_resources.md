---
id: local_resources
title: Local Resources For Tests Execution
---

Executing a test might require an external resource which is expensive to
create. For example running an iOS UI test requires an iOS simulator and it
takes relatively long time to setup it prior to test execution. When tests are
executed remotely resources initialization and allocation could be preemptively
managed by remote execution tier which is not the case for local execution. To
effectively manage such resources needed for local execution of tests there is a
separate Buck2 feature backed by `LocalResourceInfo` provider.

## `LocalResourceInfo` provider

This provider describes how to initialize and clean up a pool of homogeneous
local resources. Management of initialized resources is done by Buck2 itself
when it executes tests requiring such resources.

Fields:

- `setup` — command represented by `cmd_args` object which is executed to
  initialize a local resource. Running this command should write a JSON to
  stdout. This JSON represents a pool of local resources which are ready to be
  used.
- `resource_env_vars` — key-value mapping `{str: str}` from environment variable
  (appended to an execution command for test which is dependent on this local
  resource) to keys in JSON output of `setup` command.

Example JSON output of `setup` command:

```json
{
  "pid": 42,
  "resources": [{"socket_address": "foo:1"}, {"socket_address": "bar:2"}]
}
```

JSON keys:

- `pid` — an optional attribute which maps to a PID of a process that holds
  initialized local resources. If present, on non-Windows platforms the process
  will be sent `SIGTERM` when those resources are no longer needed. Signal
  should be handled to release any system resources related to local resources.
- `resources` — a list of resource instances, each is a mapping from a string
  alias (e.g. `socket_address`) to a value which represents resource. The number
  of concurrently running tests that require resources of the same type is
  limited by how many instances are in a list. String alias is mapped to an
  environment variable key (which will be added to a command requiring such
  resource) using a `resource_env_vars` field in `LocalResourceInfo` provider
  (see [example](#example-usage) below).

## Test Execution

### Cleanup registration protocol (Linux, opt in)

`[buck2] local_resource_cleanup_registration = true` enables a local ownership
channel for setup commands. The default is false. Buck passes a per-attempt
`BUCK2_LOCAL_RESOURCE_CLEANUP_V1` JSON capability with `version`, `socket_path`,
`attempt_id`, `consumer_pid` and `consumer_uid`. Disabled consumers explicitly
pass an empty value. Providers requiring this protocol must fail before allocation
when no authenticated consumer is available. Existing providers may ignore it.

The consumer creates a private Unix socket directory. A provider's cleanup process
connects before allocation, verifies the consumer's Linux peer PID/UID, and sends
`HELLO 1 <attempt_id> <cleanup_pid>\n`. Buck verifies the peer PID/UID and replies
`CONNECTED 1 <attempt_id>\n`. This acknowledges supervision during setup; it does
not acknowledge registration. Setup cancellation, provider death before yield,
consumer EOF and the provider's existing setup deadline must still trigger cleanup.

Successful setup includes an optional field alongside the existing `pid` and
`resources`: `"cleanup_registration": {"version": 1, "attempt_id": "..."}`.
The PID must match the authenticated cleanup process. Buck validates resources,
stores a pending cleanup lease, then sends `REGISTER 1 <attempt_id>\n`. The cleanup
process checks cancellation/deadline before replying `REGISTERED 1 <attempt_id>\n`.
Only this acknowledgment makes the resource available. The configured setup
timeout covers execution and registration together. A failed, timed out or
interrupted registration closes the channel, even after pending state was stored.

After registration, the cleanup process observes consumer EOF and signals for the
resource lifetime. Setup expiration must be disabled; it must not terminate tests
that use registered resources. Channel records are limited to 128 bytes, and
unknown, malformed or out-of-order records fail closed.

For registered protocol resources, release closes the channel and makes the pool
unavailable. Buck does not apply the legacy PID termination/escalation helper:
the independent cleanup process retains responsibility for completing its cleanup
attempt. Successful release means the request was issued, not that backend release
or process exit was verified. Legacy output without a protocol connection retains
the existing SIGTERM contract. A connection without a descriptor is an error.

The private directory and peer credentials protect the existing same-user local
execution boundary; hostile code running as that UID or root is outside it.
The attempt identifier binds one setup, rather than acting as a bearer secret.
Channel ownership cannot recover unknown backend allocation IDs or survive hard
loss of the cleanup process, enclosing cgroup or host. Providers remain responsible
for reporting their backend cleanup/retry limits.

For a general context on how tests are executed, see
[Test Execution](test_execution.md).

A decision whether certain local resource is required for specific test is made
by a test runner. List of required resources is then passed to Buck2 in
`required_local_resources` field of `ExecuteRequest2` test API protobuf message.

If resource is required for a certain test execution and test could potentially
be executed locally, `local_resources` field in test's `ExternalRunnerTestInfo`
provider is used to select appropriate `LocalResourceInfo` provider.

`ExternalRunnerTestInfo.local_resources` is a key-value mapping
`{str: ["label", None]}`. Keys represent resource types that match the values
passed from the test runner, and values are labels that should point to a target
exposing the `LocalResourceInfo` provider to be used for the initialization of
the resource of that type. If the value is `None`, it indicates that a resource
of that type will not be provided, even if the test runner requests it.

Before running a test, `setup` command from selected provider is executed and
its output is used to create a pool of resource instances. This pool is shared
across all tests pointing to the same configured target label containing
`LocalResourceInfo` provider (normally that means pool is shared for tests
requiring same resource type). A resource is acquired (with potential queuing)
from that pool prior single test is executed and is returned back to the pool
when test finished execution. After `buck2 test` command is finished, cleanup is
performed when SIGTERM is sent to each process holding a pool of resources.

## Example Usage

Define a target which has `LocalResourceInfo` provider:

```python
simulator(
  name = "my_resource",
  broker = ":broker",
)
```

where `broker` points to a runnable handling actual simulators.

Implementation of `simulator` rule would be:

```python
def _impl(ctx: AnalysisContext) -> ["provider"]:
  return [
    DefaultInfo(),
    LocalResourceInfo(
      setup = cmd_args([ctx.attrs.broker[RunInfo]]),
      resource_env_vars = { "IDB_COMPANION": "socket_address" },
    )
  ]
```

Running a `:broker` via `setup` command produces the following JSON:

```json
{
  "pid": 42,
  "resources": [{"socket_address": "foo:1"}, {"socket_address": "bar:2"}]
}
```

When Buck2 locally executes a test which requires this particular type of local
resource, it reserves one resource from the pool (e.g.
`{"socket_address": "bar:2"}`) and add environment variable representing this
resource to execution command (e.g. `IDB_COMPANION=bar:2`). In our examples
`"socket_address"` alias was substituted by `"IDB_COMPANION"` based on
`LocalResourceInfo.resource_env_vars` field.

The last part is to map a resource type to desired `LocalResourceInfo` provider.
Let's assume a test runner requires a resource of type "ios_simulator" for every
`apple_test` rule.

Pass `:my_resource` target as a dependency into `apple_test` rule:

```python
apple_test = rule(
    impl = apple_test_impl,
    attrs = {
        ...
        "_ios_simulator": attrs.default_only(attrs.dep(default = ":my_resource", providers = [LocalResourceInfo])),
        ...
    },
)
```

Actually map "ios_simulator" resource type to `:broker` target containing
`LocalResourceInfo` provider:

```python
def apple_test_impl(ctx: AnalysisContext) -> ["provider"]:
    ...
    return [
        ...
        ExternalRunnerTestInfo(
            ...
            local_resources = {
                "ios_simulator": ctx.attrs._ios_simulator,
            },
            ...
```
