---
id: configurations
title: Configurations
---

Build configurations are how Buck models building the same target in
different ways. This can include (but is not limited to):

- Target architecture
- Target OS
- Optimization level/build mode
- Compiler type/version
- Language version (e.g. C++ standard version)
- Sanitizers
- Passing arbitrary flags to build tools

When building a target, Buck always builds it in a particular
configuration. Build configurations are also sometimes called
"platforms". While technically separate, those two concepts are almost
identical.

Build configurations are [composed](../../api/build/ConfigurationInfo) of a set
of constraints and a set of values.

## Configuration constraints

Configuration constraints are enum-like constructs. Here is an example
definition:

```python
# //config/BUCK

constraint(
    name = "build_mode",
    default = "debug",
    values = [
        "debug",
        "release",
    ],
)
```

This will generate two configuration targets:
`//config:build_mode[debug]` and `//config:build_mode[release]`.

Note that currently extra contraint values can be added outside of the
main `constraint` definition. However, this is deprecated and will be
removed in the future.

```python
# //somewhere/else/BUCK

constraint_value(
    name = "release_no_debug_info",
    constraint_setting = "//config:build_mode",
)
```

Now, the three configuration targets that can be used to control the
build mode would be:

- `//config:build_mode[debug]`
- `//config:build_mode[release]`
- `//somewhere/else:release_no_debug_info`

Constraint values can also be grouped into larger logical pieces.
Assuming that we have also defined other constraints:

```python
config_setting(
    name = "dev",
    constraint_values = [
        ":build_mode[debug]",
        ":compiler[clang_21]",
        ":asan[enabled]",
    ],
)
```

Note that the prelude defines some generic constraints, e.g. under
`prelude//os:` and `prelude//cpu:`, which you might want to consider
using for interoperability. We are planning to make it easier for anyone
to customize these constraints based on their needs.

Once defined, this constraint can used in various ways, such as:

- Being passed on the command line to run a build in debug or release
  mode.
- Being "selected on" so that building in debug vs release mode has
  different effects.
- Being used for constraining compatibility (e.g. "this target can only
  be built in release mode" for a benchmark).
- Being used to "transition" part of the build (e.g. "this target and
  its dependencies are always built in release mode, regardless of the
  dependent")

## Configuration values

`config_setting` can also include values taken from the buckconfig.
These can ease a migration from a legacy buckconfig setting to a build
constraint by allowing you to `select()` (more on that later) on known
buckconfig values:

```python
config_setting(
    name = "fastmode_enabled",
    values = {
        "build.fastmode": "true",
    },
)
```

This setting will be satisfied if the associated buckconfig matches,
i.e. if the user passes `build.fastmode=true` via the `-c`/`--config`
CLI flag, or if the following is set in the cell's `.buckconfig` file:

```ini
[build]
fastmode = true
```

This feature only allows reading buckconfig values, not writing them.

They are also incompatible with
[configuration modifiers](./modifiers.md):
`--modifier :fastmode_enabled` does nothing (and will be an error in the
future).

## Using configuration: `select()`

Configurations can be used to change the build behavior based on which
value is currently active:

```python
cxx_binary(
    name = "bin",
    srcs = ["main.cpp"],
    compiler_flags = select({
        "//config:build_mode[debug]": ["-O0", "-g"],
        "//config:build_mode[release]": ["-O3"],
    }),
)
```

The above example is simplistic, and build mode compiler flags would
typically be set at the toolchain level, rather than per-target, but it
shows how build constraints can be used to change a build's behavior.

`select()` can appear in almost all attributes, and it can be composed
with other collection types. For example, the following is valid:

```python
cxx_library(
    name = "lib",
    exported_deps = [
        "//common:lib",
    ] + select({
        "//config:os[linux]": ["//linux:lib"],
        "//config:os[mac]": ["//mac:lib"],
        # `DEFAULT` is a special value that is always available.
        # In this case, we do not link against any extra libraries.
        "DEFAULT": [],
    }),
)
```

If only one condition matches, the `select()` resolves to the value of
that condition.

If multiple conditions match and all matches have the same value, the
`select()` resolves to that value.

If multiple conditions match and not all matches have the same value, then
the select will be resolved to the
"most refined" of the conditions that match. A set of constraints (as in
a `config_setting`) is said to "refine" another if it is a superset of
that other's constraints. The "most refined" of a set is then the
condition that refines all the others.

Note that `select()` is resolved during configuration. This happens
after the evaluation of the BUCK file is completed, and so Starlark code
run during BUCK file evaluation does not have access to the resolved
value. This can make it difficult to have macros that do extensive
modification or inspection of attributes (which should be done in rules
instead). However, some functions
([`select_map`](../../api/build/#select_map) and
[`select_test`](../../api/build/#select_test)) allow performing limited
operations on these objects.

## Using configuration: compatibility

Constraints can also be used to limit target compatibility. For example,
assuming that our repo supports C++20, C++23 and C++26:

```python
# Reflection is only available starting with C++26, so we require it.
cxx_library(
    name = "uses_reflection",
    exported_headers = ["foo.h"],
    target_compatible_with = ["//:cxx_standard[26]"]
)

# Deducing this is not available in C++20, so we make it incompatible.
cxx_library(
    name = "uses_deducing_this",
    exported_headers = ["foo.h"],
    target_compatible_with = select({
        "//:cxx_standard[20]": ["prelude//:none"],
        "DEFAULT": [],
    })
)
```

Target compatibility requires all transitive dependencies to be
compatible as well. In other words, a node is compatible if and only if
the node itself and all of its transitive dependencies are compatible.
In the usual cases of a dependency via `attrs.dep()`, a target's
dependency will be configured and then checked for compatibility with
the same configuration as the dependent target.

When trying to build a target with the wrong configuration (we will see
how shortly), the build will just fail (unless
`--skip-incompatible-targets` is passed).

When trying to build a set of targets using a
[pattern](./target_pattern.md) (e.g. `//some/package:` or
`//some/package/...`), Buck will simply ignore incompatible targets.

See the
[reference documentation](../../api/build/Select/#target_compatible_with)
for more information.

## Changing the build configuration

The build configuration is determined as follows:

1. A base platform is resolved:
    1. If the user passed `--target-platforms` via the CLI, use that.
    2. Else, if the target being built has a `default_target_platform`
       attribute, use that. Note that since it is used to determine the
       configuration, it is one of the few attributes that are not
       `select`able.
    3. Else, use the default (`parser.target_platform_detector_spec` in
       the `.buckconfig` file).
2. [Configuration modifiers](./modifiers.md) are applied. Those are a
   lightweight way to add constraints on an individual basis (e.g.
   "build with the default configuration/platform, except with a
   different compiler").
3. [Configuration transitions](./transitions.md) are applied. Those
   allow changing the configuration of parts of the build graph based on
   arbitrary logic (e.g. "this part of the build graph should always be
   built in release mode").

Note: The target platform mechanisms from (1) are planned to be
deprecated and will eventually be removed in favor of modifiers, which
provide more flexible and composable ways to configure builds.

The target platform resolution is not applied to all nodes in the graph.
Once the top-level nodes have been configured via the target platform
resolution, the configuration is propagated to dependencies (possibly
altered by transitions).

For example:

```sh
# Build this target with the default configuration.
buck2 build :my_target
# Build it with an entirely different configuration.
buck2 build :my_target --target-platforms //my/other:platform
# Build it with the default configuration, plus release mode.
buck2 build :my_target?release
# Equivalent to the above, but applies to all targets if multiple were built.
buck2 build :my_target -m release
```

See the [configurations for rule authors](../rule_authors/configurations.md)
page for information on how to define a platform.

Other example:

```python
java_binary(
    name = "cats",
    default_target_platform = "//platforms:windows-arm64-dev",
    deps = ["//libs:foo"],
)

java_binary(
    name = "dogs",
    default_target_platform = "//platforms:mac-x86-dev",
    deps = ["//libs:foo"],
)

java_library(
    name = "foo",
    deps = [
        "//libs:common",
    ] + select({
        "//constraints:x86": ["//libs:x86"],
        "//constraints:mac-arm64": ["//libs:mac-arm64"],
        "//constraints:windows-arm64": ["//libs:win-arm64"],
        "DEFAULT": ["//libs:generic"],
    })
)
```

When running `buck2 build //binaries:cats //binaries:dogs`, the
`//binaries:cats` binary will be built in the
`//platforms:windows-arm64-dev` configuration and the `//binaries:dogs`
binary will be built in the `//platforms:mac-x86-dev` configuration.

Each of those binaries depend on `//libs:foo`, but they will get
different versions of it as the binaries' configurations will each be
passed down to their dependencies. For `//binaries:cats`, its resolved
dependencies will include `//libs:win-arm64` and for `//binaries:dogs`,
it would contain `//libs:x86`.

Note that `//libs:common` will be built twice, once for each
configuration.

When running
`buck2 build //binaries:cats //binaries:dogs --target-platforms //platforms:mac-x86-opt`,
both `//binaries:cats` and `//binaries:dogs` will be built in the
`//platforms:mac-x86-opt` configuration, use the same dependencies,
which would only be built once.

## Configurations and output paths

Since a target may appear within a build in multiple different
configurations, output paths cannot be derived based on just targets (as
multiple actions would map to the same outputs). For this reason, the
target and the configuration are encoded into output paths. The
configuration is currently represented as a hash of its values (a
"hashed buck-out").

## Target platform vs execution platform

Buck distinguishes two kinds of platforms: target platforms and execution
platforms. Target platforms are the platforms where your code runs,
and execution platforms are the ones used to run compilers and build tools
during the build process.
These are distinct because build tools typically need a different
configuration. For example, when you are building an app targeting Android
OS, you will still need to build a compiler for Linux/Mac/Windows. You may
also want to build the said compiler in release mode for faster
builds even when building the Android app in development mode.

For this reason, Buck requires both _target_ platforms and _execution_
platforms to be defined. The execution platforms are specified via the
`build.execution_platforms` value in `.buckconfig`.

## Queries

### Getting configuration constraints from its hash

Build configurations are uniquely identified by their hash, which is not
human friendly.

To determine what constraints are part of a configuration, run
`buck2 cquery //...` sot that Buck will discover all existing
configurations, then run `buck2 audit configurations`.

This will list all available configurations and print their composing
contraints.

### `cquery` and `uquery`

One way to understand the effect that a configuration has is via the
`cquery` and `uquery` commands. The `cquery` command will compute the
appropriate configuration for a target and display a version of that
target's attributes with the configuration applied. The `uquery` command
will not apply a configuration.

Here is a heavily trimmed version of the outputs of invoking `uquery`
and `cquery` on `//buck2/app/buck2_core:buck2_core`.

```sh
> buck2 uquery -A '"//buck2/app/buck2_core:buck2_core"'
{
  "fbcode//buck2/app/buck2_core:buck2_core": {
    "buck.type": "rust_library",
    "buck.package": "fbcode//buck2/app/buck2_core:TARGETS",
    "name": "buck2_core",
    "visibility": [
      "PUBLIC"
    ],
    "deps": {
      "fbsource//third-party/rust:anyhow",
      "fbsource//third-party/rust:arc-swap",
      "fbsource//third-party/rust:blake3",
      "fbsource//third-party/rust:compact_str",
      "fbsource//third-party/rust:dashmap",
      {
        "__type": "selector",
        "entries": {
          "DEFAULT": [],
          "ovr_config//os:windows": [
            "fbsource//third-party/rust:common-path"
          ]
        }
      },
      {
        "__type": "selector",
        "entries": {
          "DEFAULT": [],
          "ovr_config//os:linux": [
            "fbsource//third-party/rust:nix"
          ]
        }
      },
    },
  }
}
```

```sh
> buck2 cquery -A '"//buck2/app/buck2_core:buck2_core"'
{
  "fbcode//buck2/app/buck2_core:buck2_core (ovr_config//platform/linux:<OMITTED>)": {
    "buck.type": "rust_library",
    "buck.package": "fbcode//buck2/app/buck2_core:TARGETS",
    "buck.target_configuration": "ovr_config//platform/linux:<OMITTED>",
    "buck.execution_platform": "fbcode//buck2/platform/<OMITTED>",
    "name": "buck2_core",
    "visibility": [
      "PUBLIC"
    ],
    "deps": [
      "fbsource//third-party/rust:anyhow (ovr_config//platform/linux:<OMITTED>)",
      "fbsource//third-party/rust:arc-swap (ovr_config//platform/linux:<OMITTED>)",
      "fbsource//third-party/rust:blake3 (ovr_config//platform/linux:<OMITTED>)",
      "fbsource//third-party/rust:compact_str (ovr_config//platform/linux:<OMITTED>)",
      "fbsource//third-party/rust:dashmap (ovr_config//platform/linux:<OMITTED>)",
      "fbsource//third-party/rust:nix (ovr_config//platform/linux:<OMITTED>)"
    ]
}
```

The `cquery` output has additional `buck.target_configuration` and
`buck.execution_platform` attributes which tell you what the target is
being built for and what it's being built on, respectively. `uquery`
doesn't have those.

The deps in `uquery` also have a number of selects; these indicate that
the `common-path` dependency should only be included when building for
Windows, while the `nix` dependency is needed only for Linux. In
`cquery` that distinction has been resolved; because the target has been
configured for Linux, the `nix` dependency is present and
indistinguishable from any other, while the `common-path` dependency is
gone.

## Execution groups

By default, each target resolves a single execution platform. Execution
groups let a rule resolve one execution platform per group and choose the
group that each action runs in. See
[Execution groups](../rule_authors/execution_groups.md).

## See also

- [Configuration modifiers](./modifiers.md)
- [Configuration transitions](./transitions.md)
- [Configurations for rule authors](../rule_authors/configurations.md)
- [Configuration transitions for rule authors](../rule_authors/configuration_transitions.md)
- [Execution groups](../rule_authors/execution_groups.md)
