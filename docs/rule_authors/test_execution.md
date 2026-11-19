---
id: test_execution
title: Test Execution
---

import { FbInternalOnly, OssOnly } from
'docusaurus-plugin-internaldocs-fb/internal';

Test execution in Buck2 is a collaboration with a separate test runner process.

<OssOnly>

In its open-source build, Buck2 ships with a built-in simplistic test runner.

This test runner receives the commands defined by `ExternalRunnerTestInfo` and
simply executes them. Exit code zero means the test passed, and one means it
failed.

Users can of course develop their own test runners. Look at
`fbcode/buck2/app/buck2_test_runner` as a sample. For comparison, here's how
it's used at Meta:

</OssOnly>

At Meta, this test runner is <OssOnly>Tpx</OssOnly>
<FbInternalOnly>[Tpx](https://www.internalfb.com/intern/wiki/TAE/tpx/)</FbInternalOnly>.

Tpx has a large number of responsibilities when used with Buck2, which can be
grouped as follows:

- **Translation**:
  - Understands the output formats of various supported test frameworks. This is
    used to identify test cases and collect test results.
  - Understands, to an extent, the input formats. For example, given a test
    case, Tpx can identify what command needs to run to execute just that test.
- **Orchestration**:
  - Interacts with Test Infra to discover what tests should run, under a number
    of configurations.
  - Separates listing of tests (identifying what tests exists in a test target)
    and execution (running specific tests within that target).
  - Coordinates the execution of tests. For example, it may request retries, or
    choose to bundle multiple tests in a single execution (or not).
  - Reports test results to Test Infra as well.

In Buck2, rules interact with the test runner via a provider called
`ExternalRunnerTestInfo`.

## Anatomy of a test run

When a user runs `buck2 test $targets`:

- Buck2 identifies all matching targets that have an `ExternalRunnerTestInfo`.
- Buck2 builds all the artifacts referenced by those targets (this will likely
  change eventually to build them only if they are used).
- Buck2 then notifies the test runner that those tests exist. Currently, the
  test runner receives a subset of `ExternalRunnerTestInfo`.
- The test runner can request command execution from Buck2 to list and execute
  tests.
- When it receives command results from Buck2, the test runner may fire off
  events that the end-user will see (such as test results), upload logs
  externally, request further executions, and so on.

:::note

If more than one target is being built, test building and execution will proceed
concurrently.

:::

## Information available on `ExternalRunnerTestInfo`

As noted, rules communicate their testing capabilities via
`ExternalRunnerTestInfo`. There are a number of fields available on
`ExternalRunnerTestInfo` to control how a given target is tested, as detailed in
the following sub-sections.

### Fields exposed to the test runner

The following list shows what is available in `ExternalRunnerTestInfo`, with
which the test runner can interact:

- `type` - a string key that defines the type of test this is. <FbInternalOnly>
  Tpx uses this internally to choose a translator. Examples include `gtest`,
  `apple_test`, `custom`. Note that Tpx also allows labels to influence the
  orchestrator selection. </FbInternalOnly>
- `command` and `env` - respectively, a list and a key-value mapping of
  arguments. <FbInternalOnly>These are the inputs to translation in
  Tpx.</FbInternalOnly> They are not always visible to the test runner (for more
  details, see
  [Verbatim arguments and handles](#verbatim-arguments-and-handles), below).
- `labels` - a set of string labels to pass to the test runner. <FbInternalOnly>
  They have no meaning to Buck2, but some labels have impact on translation in
  Tpx. </FbInternalOnly>
- `contacts` - a list of contacts for the tests; usually oncalls.
- `executor_overrides` - a key-value mapping of executor configurations that the
  test runner can use when requesting execution from Buck2.
- `local_resources` - a key-value mapping from resource type to optional
  `LocalResourceInfo` provider. Provider is used for initialization of that
  resource type. If the value is `None` resource type is ignored even though
  test runner required it. For context see
  [Local Resources For Tests Execution](local_resources.md).

### Fields pertinent for Remote Execution

For compatibility with Remote Execution (RE), there are two fields that rules
should set in their `ExternalRunnerTestInfo` if they should be run on RE:

- `use_project_relative_paths` - if `true` (the default is
  <FbInternalOnly>`false`</FbInternalOnly> <OssOnly>`true`</OssOnly>), Buck2
  will produce relative paths. If not, it'll produce absolute paths.
- `run_from_project_root` - if `true` (the default is
  <FbInternalOnly>`false`</FbInternalOnly> <OssOnly>`true`</OssOnly>), tests
  will run from the project root (their `cwd` will be the project root, which is
  the same as all build commands). If `false`, it'll be the cell root.

Note that passing `--unstable-allow-all-tests-on-re` to `buck2 test` will
override those fields and set them to `true`, since they are a pre-requisite to
run on RE. In contrast, passing `--unstable-allow-compatible-tests-on-re` will
only allow tests that already set both those fields to `true` to execute on RE.

Also note that when `executor_overrides` are set, if an executor override is
used and results in execution on RE, it'll happen on RE unconditionally.
Therefore, it's a good idea to set those fields if RE-only executor overrides
are provided.

## Verbatim arguments and handles

As noted above, the test runner only interacts with a subset of arguments
provided by rules in `ExternalRunnerTestInfo`. The reason for this is that the
test runner doesn't get to access, for example, artifacts, that Buck2 knows
about.

Consider the following example:

```python
binary = ctx.attrs.dep[RunInfo]
test_info = ExternalRunnerTestInfo(command = [binary, "run-tests"], ...)
```

When Buck2 actually runs this command, `binary` is expanded to a path (and
possibly to more args). Buck2 would also account for any hidden arguments and
make those available where the command is executed. It is important for Buck2 to
retain this capability when running with the test runner.

To that end, all non-trivial arguments present in `command` (and in the values
of `env`), such as `cmd_args` or `RunInfo`, are exposed to the test runner as
opaque handles, and simple string arguments are passed as-is to the test runner.

This means that the test runner would see the command described above as:

```python
[ArgHandle(index = 0), Verbatim("foobar")]
```

When requesting execution from Buck2, the test runner can use the `ArgHandle`
and Buck2 will swap it back for the underlying value that was set on the
provider.

This allows the test runner to introspect and modify parts of the command lines
it receives, as long as it doesn't need to access the actual text value of
non-verbatim arguments. Usually, this works out to be sufficient (or can be made
sufficient with a bit of refactoring in the test runner).

## Execution Configurations

By default, tests execute using the execution configuration of the associated
target. This is the execution configuration that would be used for run actions
(`ctx.actions.run`) declared in the same target. This is a default that actually
makes little sense but works out as long as cross-compiling is not the norm.

<FbInternalOnly>

That said, it's easy to see where this breaks down.

For example:

- For iOS tests, the execution platform for builds needs to be Xcode (local or
  RE Mac).
- For test listing, Xcode is not needed (it's preferable to do it on RE Linux
  where capacity is cheaper).
- To run the tests, a simulator is required.

</FbInternalOnly>

To support this, `ExternalRunnerTestInfo` allows specifying override platforms,
which are given a name. The test runner can request execution on them by passing
their name when it sends execution requests to Buck2, as shown in the following
code:

```python
ExternalRunnerTestInfo(
  executor_overrides = {
      "ios-simulator": CommandExecutorConfig(
          local_enabled = False,
          remote_enabled = True,
          remote_execution_properties = {
              "platform": "ios-simulator-pure-re",
              "subplatform": "iPhone 8.iOS 15.0",
              "xcode-version": "xcodestable",
          },
          remote_execution_use_case = "tpx-default",
      ),
      "static-listing": CommandExecutorConfig(local_enabled = True, remote_enabled = False),
  },
  ...
)
```

The default execution platform can also be overridden:

```python
ExternalRunnerTestInfo(
  default_executor = CommandExecutorConfig(
    local_enabled = False,
    remote_enabled = True,
    remote_execution_properties = {
        "platform": "ios-simulator-pure-re",
        "subplatform": "iPhone 8.iOS 15.0",
        "xcode-version": "xcodestable",
    },
    remote_execution_use_case = "tpx-default",
  ),
  ...
)
```

Instead of `default_executor`, a rule with
[execution groups](execution_groups.md) can pass `exec_group` to
`ExternalRunnerTestInfo` or `InternalRunnerTestInfo` to run the test on the
executor of that group. If both are set, `default_executor` wins. An executor
override that the test runner picks still takes precedence over either.

## Working Directory

<OssOnly>
Tests can be run from the cell root by setting `run_from_project_root = False`.
</OssOnly>

<FbInternalOnly>
As noted above, tests run from the cell root unless `run_from_project_root` is set.
</FbInternalOnly>

To produce paths relative to the cell root for use by tests, use
`relative_to(ctx.label.cell_root)` on `cmd_args`.

<FbInternalOnly>

## Caching Test Listings

Running tests typically starts with a **listing** step: the test binary is
invoked to discover what test cases exist. For large test suites this can be
time-consuming. Buck2 caches listing results at two levels so that repeated test
runs can skip the listing step entirely.

### How it works

When the test runner (Tpx) requests a listing execution, it marks the listing as
**cacheable** unless the test target opts out. Cacheable listings benefit from
two layers of caching:

1. **DICE (in-memory) caching** — Within a single Buck2 daemon session,
   cacheable listing results are stored in the DICE computation graph. If you run
   `buck2 test` on the same target again without changes, the listing is skipped
   entirely (it won't appear in `buck2 log what-ran` output).

2. **Remote Execution action cache** — Listing results are also stored in the RE
   action cache. This means caching survives daemon restarts: after a `buck2
   kill`, the next test run can fetch the listing result from the remote cache
   instead of re-executing the test binary. This is especially effective on CI,
   where many tests are re-run frequently without changes.

Only **successful** listings (exit code 0) are cached. Failed listings are
always re-executed so that transient failures don't get stuck in the cache.

### Local listing cache uploads

When tests run locally (not on RE), listing results can still be uploaded to the
remote action cache for other builds to benefit from. This is controlled by the
`test.allow_cache_uploads` buckconfig:

```ini
[test]
allow_cache_uploads = true
```

### Opting out of listing caching

If a test target produces non-deterministic listings (for example, tests that are
dynamically generated at listing time), caching would return stale results. To
opt out, add the `tpx:listing_uncacheable` label to the target's
`ExternalRunnerTestInfo`:

```python
ExternalRunnerTestInfo(
    command = ["my_test_binary"],
    type = "my_test_type",
    labels = ["tpx:listing_uncacheable"],
)
```

When this label is present, the listing is executed every time and results are
never uploaded to the action cache.

### Configuration

The following buckconfigs affect listing caching behavior:

| Buckconfig | Effect |
|---|---|
| `test.remote_cache_enabled` | Controls whether the RE action cache is checked for listing results. |
| `test.allow_cache_uploads` | Controls whether locally-executed listing results are uploaded to the RE action cache. |

</FbInternalOnly>
