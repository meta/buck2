---
id: execution_groups
title: Execution Groups
---

A configured target normally has one
[execution platform](configurations.md#execution-platforms), and every action
the target registers runs there. Execution groups let one target use several
execution platforms. A rule declares groups, each group gets its own execution
platform, and each action says which group it runs in.

Use an execution group when different actions of one target need different
machines. For example:

- Resource compilation needs a macOS tool, but packaging can run on Linux.
- Compilation can run remotely, but the link must run locally.

Without groups, a single macOS-only tool moves the whole target to macOS. The
usual workaround is to split the target in two. With groups, the macOS-only
tool constrains only the group that runs it.

## Declaring groups

Create a group with `exec_group()` and assign it to a top-level global. The
name of the global is the name of the group:

```python
Link = exec_group()
```

`exec_group()` takes no arguments. Groups are values of type `ExecGroup`, so
you can define them in a shared `.bzl` file and `load()` them. Loading a group
under another name does not rename it. A group is identified by its name and
the file that defines it.

A rule lists the groups it uses in `exec_groups`:

```python
my_binary = rule(
    impl = _my_binary_impl,
    attrs = {...},
    exec_groups = [DefaultExecGroup, Link],
)
```

`DefaultExecGroup` is the built-in default group. Its platform is the target's
usual execution platform. Every exec dep, toolchain dep and action that does
not name a group is in the default group.

- A rule without named groups uses the default group implicitly and needs no
  changes.
- A rule with named groups can use the default group only if it lists
  `DefaultExecGroup` in `exec_groups`. Leave it out to make sure that every
  action names a group.
- Every group in `exec_groups` must have a distinct name.
- The name `DefaultExecGroup` is reserved: `DefaultExecGroup = exec_group()` is
  an error.
- A rule with named groups and `uses_plugins` must list `DefaultExecGroup`,
  because plugins are deps of the default group.

## Putting deps in a group

Pass `exec_group` to `attrs.exec_dep()` or `attrs.toolchain_dep()`. The dep is
configured for that group's platform, and it constrains only that group's
platform resolution:

```python
Resources = exec_group()

attrs = {
    "_compiler": attrs.exec_dep(default = "//tools:compiler", exec_group = DefaultExecGroup),
    "_linker": attrs.exec_dep(default = "//tools:linker", exec_group = Link),
    "_resources": attrs.toolchain_dep(default = "//toolchains:resources", exec_group = Resources),
}
```

In a rule with named groups, every `attrs.exec_dep()` and
`attrs.toolchain_dep()` must pass `exec_group`, including nested ones such as
`attrs.list(attrs.exec_dep(exec_group = Link))`. Tools for the default group
pass `exec_group = DefaultExecGroup`. A missing `exec_group` is an error when
the `.bzl` file loads, so a forgotten mark cannot silently put a tool in the
default group. `exec_group = None` counts as no mark, which lets
shared helpers pass an optional group through.

`attrs.arg()` also takes `exec_group`. It applies to the tools that the arg
refers to with `$(exe ...)`, `$(location_exec ...)` and
`$(location_toolchain ...)`. Other macros, such as `$(location ...)`, are not
affected. In a rule with named groups, an `attrs.arg()` without `exec_group`
may hold flags but not tools. A tool macro in a target's value is an error
when the target is created, and a tool macro in the attribute's `default` is an
error when the `.bzl` file loads. The fix goes in the rule, not in the BUCK
file:

```python
attrs = {
    "link_cmd": attrs.arg(exec_group = Link),  # may refer to tools
    "link_flags": attrs.list(attrs.arg(), default = []),  # flags only
}
```

Each attribute is in one group. If two groups need the same tool, use two
attributes that point at the same target. When both groups resolve to the same
platform, Buck2 configures the tool once.

## Constraining a group

A target's `exec_compatible_with` applies to every group. To let a target
constrain a single group, the rule adds an `attrs.exec_compatible_with()`
attribute for that group:

```python
attrs = {
    "link_exec_compatible_with": attrs.exec_compatible_with(exec_group = Link, default = []),
}
```

The constraints a target sets in this attribute apply only to `Link`, in
addition to the target's `exec_compatible_with`. The value can be a list or a
`select()`. With `exec_group = DefaultExecGroup`, the constraints apply only to
the default group. A rule may have at most one such attribute per group, and it
cannot be nested inside another attribute.

## Running actions in a group

Pass `exec_group` to `ctx.actions.run()`:

```python
ctx.actions.run(cmd, category = "link", exec_group = Link)
```

`ctx.actions.write()` and `ctx.actions.write_json()` take `exec_group` too.
A write renders artifact paths for its platform, for example with that
platform's path separator, so write a file in the group of the actions that
read it.

Actions without an `exec_group` parameter, such as `copy_file` and
`symlinked_dir`, are always registered in the default group, even in a rule
that does not list `DefaultExecGroup`. `aquery` shows `DefaultExecGroup` for
them. They do not run a command on the platform, so this is harmless.

A `run`, `write` or `write_json` without `exec_group` (or with `None`) runs in
the default group. In a rule that declares named groups but not
`DefaultExecGroup`, that is an error. An action in a group that the rule does
not declare fails at analysis with
``Execution group `Other` is not declared by this rule. Declared groups: [...]``.
An action in a group that has no platform (see
[How resolution works](#how-resolution-works)) fails at the call, and the
error names the group and the action.

Run each action with tools from attributes of its own group. Buck2 does not
check this; see [Limitations](#limitations). Outputs of the target's own
actions can be used by actions in any group.

`ctx.actions.run()` inside a [dynamic output](dynamic_dependencies.md) takes
`exec_group` in the same way.

## Tests

`ExternalRunnerTestInfo` and `InternalRunnerTestInfo` take `exec_group`. The
test runs on the executor of that group when the test runner picks no executor
override:

```python
ExternalRunnerTestInfo(type = "custom", command = [...], exec_group = TestRun)
```

- When the test info is created, the executor of its group is recorded in
  `default_executor` if the creating rule declares that group and the group has
  a platform. A rule that returns another target's test info, such as `alias`,
  then keeps the executor of the creating rule's group.
- Otherwise nothing is recorded, and the group, or the default group for a test
  info without `exec_group`, is resolved against the tested target when the
  test runs. For example, a forwarding rule that declares named groups but not
  `DefaultExecGroup` cannot forward a test info that uses the default group:
  the build succeeds, and `buck2 test` fails because the tested target's rule
  does not declare the default group.
- A test info that names a group its rule does not declare is created without
  error. The build succeeds, and the test fails at `buck2 test`.
- If a test info sets both `default_executor` and `exec_group`,
  `default_executor` wins.
- In a rule that declares named groups but not `DefaultExecGroup`, a test info
  must set `exec_group` or `default_executor`. Otherwise its creation fails.
  `executor_overrides` alone is not enough, because the test runs in the
  default group whenever the test runner picks no override.

See [Test Execution](test_execution.md) for executors and overrides.

## Anon targets

`ctx.actions.anon_target()` and `ctx.actions.anon_targets()` take `exec_group`.
The anon target then uses that group's platform instead of the target's
default platform:

```python
anon = ctx.actions.anon_target(_link_rule, {"linker": ctx.attrs._linker}, exec_group = Link)
```

- Exec deps passed in must be built for the anon target's platform, so pass the
  group of the attribute the tool comes from. The same check applies to tools
  in args passed in: always to the tools of an arg with a named `exec_group`,
  and to every arg tool when the calling rule declares named groups or the
  anon target is created with `exec_group`.
- The rule given to `anon_target()` must not declare named execution groups
  (`exec_groups = [DefaultExecGroup]` is accepted). Anon targets have no groups
  of their own.
- In a rule that declares named groups but not `DefaultExecGroup`, `exec_group`
  is required.
- Creating an anon target in a group that has no platform fails at the call.

See [Anonymous Targets](anon_targets.md).

## How resolution works

Buck2 resolves a platform for each group independently, with the same
algorithm it uses for a target's execution platform. It picks the first
registered execution platform that satisfies the group's constraints and on
which every dep of the group is compatible, or uses the fallback. Only the
inputs differ:

| Group               | Constraints                                                                                               | Deps                                                                                                               |
| ------------------- | --------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------ |
| Default             | The target's `exec_compatible_with`, plus its `attrs.exec_compatible_with(exec_group = DefaultExecGroup)` | Exec deps, toolchain deps and arg tools in the default group, and plugins                                          |
| Named group `G`     | The target's `exec_compatible_with`, plus its `attrs.exec_compatible_with(exec_group = G)`                | Only exec deps, toolchain deps and arg tools with `exec_group = G`                                                 |

- The deps of one group do not affect any other group, including the default
  group.
- A group does not prefer the default group's platform. A named group already
  gets the target's `exec_compatible_with`, but not the default group's deps.
  A group that must land on the same platform as the default group needs its
  own constraints (for example its
  `attrs.exec_compatible_with(exec_group = ...)`) or deps that rule out the
  other platforms. Otherwise it takes the first compatible registered
  platform.
- Every declared group is resolved, even one with no deps.
- A rule without named groups resolves a single execution platform, as
  described in [Configurations](configurations.md#execution-platform-resolution).
- When `build.execution_platforms` is not set, every group gets the legacy
  execution platform, the same one as the target.

When no platform fits a group, the group follows the `fallback` of the
`ExecutionPlatformRegistrationInfo` that registers the execution platforms, as
the default group does:

- `fallback = "error"`: the target fails to configure, with an error that
  names the group.
- `fallback` set to an `ExecutionPlatformInfo`: the group uses that platform.
- No `fallback`, or `fallback = "use_unspecified"`: the group has no platform.
  `buck.exec_group_platforms` shows `<NONE>` for it, and the target still
  configures and builds. An action or anon target in the group fails at the
  call, and a test in the group fails when it runs. The group's deps are
  configured without an execution platform, so a dep that checks its
  configuration, for example with `target_compatible_with`, fails to
  configure.

## Example

A rule that compiles in the default group and links in a `Link` group:

```python
# my_binary.bzl
Link = exec_group()

def _my_binary_impl(ctx):
    obj = ctx.actions.declare_output("main.o")
    ctx.actions.run(
        cmd_args(ctx.attrs._compiler[RunInfo], ctx.attrs.src, obj.as_output()),
        category = "compile",
    )
    out = ctx.actions.declare_output(ctx.label.name)
    ctx.actions.run(
        cmd_args(ctx.attrs._linker[RunInfo], obj, out.as_output()),
        category = "link",
        exec_group = Link,
    )
    return [DefaultInfo(default_output = out)]

my_binary = rule(
    impl = _my_binary_impl,
    attrs = {
        "link_exec_compatible_with": attrs.exec_compatible_with(exec_group = Link, default = []),
        "src": attrs.source(),
        "_compiler": attrs.exec_dep(default = "//tools:compiler", exec_group = DefaultExecGroup),
        "_linker": attrs.exec_dep(default = "//tools:linker", exec_group = Link),
    },
    exec_groups = [DefaultExecGroup, Link],
)
```

Targets do not need to know about groups. They only set the attributes the
rule exposes:

```python
# BUCK
load(":my_binary.bzl", "my_binary")

my_binary(
    name = "app",
    src = "main.c",
)

my_binary(
    name = "app_local_link",
    src = "main.c",
    # Only the link must run on a local platform.
    link_exec_compatible_with = ["//config:local"],
)
```

## Inspecting groups

The output below comes from the example above, with two registered execution
platforms and a compiler that is compatible only with remote execution:

```python
# config/BUCK
load(":defs.bzl", "constraint_setting", "constraint_value", "platform", "platforms")

constraint_setting(name = "os")
constraint_value(name = "linux", setting = ":os")
constraint_setting(name = "executor")
constraint_value(name = "local", setting = ":executor")
constraint_value(name = "remote", setting = ":executor")

platform(name = "linux_local", configuration = [":linux", ":local"])
platform(name = "linux_remote", configuration = [":linux", ":remote"], remote = True)

# Resolution picks the first platform that fits, so the order matters.
platforms(name = "platforms", platforms = [":linux_remote", ":linux_local"])
```

```python
# tools/BUCK
load(":defs.bzl", "tool")

tool(name = "compiler", target_compatible_with = ["//config:remote"], visibility = ["PUBLIC"])
tool(name = "linker", visibility = ["PUBLIC"])
```

```ini
# .buckconfig
[build]
execution_platforms = root//config:platforms
```

These are not prelude rules. `constraint_setting`, `constraint_value`,
`platform` and `platforms` come from a small local `config/defs.bzl`, like the
one in Buck2's
[execution group tests](https://github.com/facebook/buck2/blob/main/tests/core/configurations/test_execution_groups_data/config/defs.bzl).
`platform` returns an `ExecutionPlatformInfo` and a `PlatformInfo` for its
constraints. Its `remote` attribute is a test helper: `remote = True` gives the
platform a remote-only executor, and the default is a local-only one.
`platforms` returns an `ExecutionPlatformRegistrationInfo`. `tool` comes from a
local `tools/defs.bzl` and is a rule that returns a `RunInfo`:

```python
tool = rule(impl = lambda ctx: [DefaultInfo(), RunInfo(args = cmd_args("true"))], attrs = {})
```

`buck2 cquery` shows the platform of each named group in
`buck.exec_group_platforms`, or `<NONE>` for a group that has no platform. The
attribute is present only for rules that declare named groups.
`buck.execution_platform` is still the platform of the default group.

```sh
$ buck2 cquery //app:app_local_link --target-platforms //config:linux_local --output-attribute 'buck.execution_platform|buck.exec_group_platforms'
{
  "root//app:app_local_link (root//config:linux_local#...)": {
    "buck.execution_platform": "root//config:linux_remote",
    "buck.exec_group_platforms": {
      "Link": "root//config:linux_local"
    }
  }
}
```

`buck2 aquery` shows the group of each action in `buck.exec_group`, as
`DefaultExecGroup` or the group's name. Only actions of rules that declare named
groups have this attribute.

```sh
$ buck2 aquery 'deps(//app:app_local_link)' --target-platforms //config:linux_local --output-attribute 'category|buck.exec_group'
{
  ...
  "(target: `root//app:app_local_link (root//config:linux_local#...)`, id: `0`)": {
    "category": "compile",
    "buck.exec_group": "DefaultExecGroup"
  },
  "(target: `root//app:app_local_link (root//config:linux_local#...)`, id: `1`)": {
    "category": "link",
    "buck.exec_group": "Link"
  }
}
```

`buck2 audit execution-platform-resolution` prints one section per group, with
that group's platform, exec deps, toolchain deps, and the platforms it skipped
and why. The default group's section is headed `Execution platform:`, and each
named group's section is headed ``Execution group `G`:``. Use it when a group
resolves to an unexpected platform or to none.

```sh
$ buck2 audit execution-platform-resolution //app:app_local_link --target-platforms //config:linux_local
Checking each target against execution platforms defined by root//config:platforms
root//app:app_local_link (root//config:linux_local#...):
  Execution platform: root//config:linux_remote
    Execution platform configuration: root//config:linux_remote#...
    Execution deps:
      root//tools:compiler (root//config:linux_remote#...)
    Toolchain deps:
    Configuration deps:
  Execution group `Link`: root//config:linux_local
    Execution deps:
      root//tools:linker (root//config:linux_local#...)
    Toolchain deps:
    Skipped root//config:linux_remote
      exec_compatible_with requires `root//config:local` but it was not satisfied
```

## Limitations

- Buck2 does not check that an action only runs tools from its own group. An
  action that runs a tool built for another group's platform may fail when it
  executes.
- A toolchain dep joins a group as a whole. Every exec dep of the toolchain
  constrains that group's platform, even tools the group never runs.
- Toolchain rules and configuration rules cannot declare execution groups, not
  even `[DefaultExecGroup]`. The exec deps of a toolchain belong to the group
  that the depending rule puts the toolchain in.
- A rule that lists only named groups still resolves a default platform, and
  `buck.execution_platform` still shows it. No action, test info or anon target
  can run there; see [Running actions in a group](#running-actions-in-a-group)
  for the actions that are registered there anyway.
