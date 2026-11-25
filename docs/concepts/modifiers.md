---
id: modifiers
title: Configuration modifiers
---

import { FbInternalOnly } from 'docusaurus-plugin-internaldocs-fb/internal';

A configuration modifier changes one thing about how a target is built: "in
release mode", "with AddressSanitizer", "with this compiler". It can be given
on the command line, on a target, or for a whole directory in a
[`PACKAGE` file](package_files.md).

```sh
buck2 build //app:server -m release
```

In the terms of [Configurations](configurations.md), a modifier is a constraint
value. Adding it to a target's configuration replaces the value that the
configuration had for the same constraint. A `select()` on that constraint, in
the target, in the libraries it depends on or in their toolchains, then takes
the branch of the new value.

## An example

A project declares a constraint with the build modes it has:

```python
# config/BUCK
constraint(
    name = "build_mode",
    default = "debug",
    values = [
        "debug",
        "release",
    ],
)
```

and its C++ toolchain chooses compiler flags by it
([Set up configuration modifiers](../users/how_tos/modifiers_setup.md) has the
whole toolchain):

```python
# toolchains/BUCK
system_cxx_toolchain(
    name = "cxx",
    cxx_flags = select({
        "root//config:build_mode[debug]": ["-O0", "-g"],
        "root//config:build_mode[release]": ["-O2", "-DNDEBUG"],
    }),
    ...
)
```

`buck2 build //app:server` builds with the default, `debug`.
`buck2 build //app:server -m 'root//config:build_mode[release]'` compiles the
server and the libraries it depends on with `-O2 -DNDEBUG`. Once the project
declares `release` as an alias for that constraint value, `-m release` does the
same. The two builds have different configurations, so their outputs are in
different directories of `buck-out` and neither replaces the other.

## Enabling modifiers

<FbInternalOnly>

Modifiers are already enabled in Meta's repositories. This section is for
projects outside Meta.

</FbInternalOnly>

A project has to enable modifiers before it can use them, by calling
`set_cfg_constructor` in the `PACKAGE` file of the project root.
[Set up configuration modifiers](../users/how_tos/modifiers_setup.md) has the
call.

The call is needed because Buck2 itself only collects the modifiers that apply
to a target. Combining them with the target's platform into a configuration is
the work of a Starlark function, the _cfg constructor_, and
`set_cfg_constructor` says which function that is. The prelude has one, and it
implements what this page describes.

:::caution

In a project that has not called `set_cfg_constructor`, modifiers are ignored.
`-m release` and a `modifiers` attribute are accepted, and the target is built
in its default configuration.

:::

## Where modifiers come from

The configuration of a target named on the command line starts from its
[platform](configurations.md#changing-the-build-configuration). Modifiers are
then applied in this order:

1. The modifiers of `PACKAGE` files, from the project root down to the target's
   directory. See
   [Add configuration modifiers to entire directories](../users/how_tos/modifiers_package.md).
2. The `modifiers` attribute of the target. See
   [Add configuration modifiers to a specific target](../users/how_tos/modifiers_target.md).
3. The modifiers on the command line, in the order they are written. See
   [Specify configuration modifiers from the CLI](../users/how_tos/modifiers_cli.md).

A later modifier wins over an earlier one for the same constraint. Modifiers
for different constraints combine: with `-m release -m asan`, both apply.

Modifiers are read for the targets that the command names, and not for their
dependencies. A library is built in the configuration of the target that
depends on it, whatever the library's own `PACKAGE` files and `modifiers`
attribute say. To always build a dependency in a particular way, use a
[transition](transitions.md).

## What can be a modifier

- A constraint value: `root//config:build_mode[release]` for a value of a
  `constraint`, or the target of a `constraint_value`. A `config_setting` that
  has exactly one constraint value stands for that value.
- A [conditional modifier](#conditional-modifiers).
- On the command line, an alias for one or more of the above. Aliases are
  declared in the `set_cfg_constructor` call.

A modifier stands for exactly one constraint value. A `config_setting` with
several constraint values and a `platform` cannot be modifiers.

Shells such as zsh treat `[`, `]` and `?` as parts of a file name pattern, so
quote a modifier or a target that contains them.

## Conditional modifiers

A conditional modifier adds a constraint value only in some configurations. It
is a target, made with the prelude's `conditional_modifier`:

```python
load("@prelude//cfg/modifier:conditional_modifier.bzl", "conditional_modifier")

# AddressSanitizer in debug builds, and nothing in others.
conditional_modifier(
    name = "asan_in_debug",
    modifier = {
        "root//config:build_mode[debug]": "root//config:sanitizer[address]",
        "DEFAULT": None,
    },
)
```

A key matches when the configuration, as the other modifiers leave it, has the
key's constraint values. The first key that matches is used, and `DEFAULT` when
none does. `buck2 build //app:server -m root//config:asan_in_debug` builds
with the sanitizer, and adding `-m release`, before or after it, builds
without.

A value can be a constraint value, `None` for no change, another conditional
modifier, or a nested dictionary. All the values of one conditional modifier
must be for the same constraint.

## Modifiers and build tools

A build runs tools: compilers, code generators, scripts. A tool that is itself
built from source, as an
[execution dependency](../rule_authors/configurations.md#execution-deps), is
built for the machine that runs it, in a configuration of its own. Modifiers
from the command line do not reach it. Building the server with `-m release`
does not rebuild the code generator in release mode, and building it with
`-m asan` does not put a sanitizer into the compiler.

The modifiers of a tool's own `PACKAGE` files and `modifiers` attribute apply to
it only for constraints declared with `execution_modifier = True`:

```python
constraint(
    name = "build_mode",
    default = "debug",
    execution_modifier = True,
    values = [
        "debug",
        "release",
    ],
)
```

## When to use modifiers

Use a modifier to choose between variants of a build that the project has
prepared: the build mode, a sanitizer, the compiler, the operating system or
CPU to build for.

- A modifier, and not a `--config` (`-c`) value read with `read_config`. A
  `.buckconfig` value applies to a whole command, so one command cannot build
  two variants, and changing it re-evaluates every `BUCK` file that reads it. A
  modifier is part of a target's configuration:
  `buck2 build '//app:server?debug' '//app:server?release'` builds both at once.
- A modifier, and not a platform for every combination. A platform passed with
  `--target-platforms` replaces the whole configuration, so a project needs
  one for each combination of build mode, sanitizer and so on. Modifiers change
  one constraint each and combine.
- A [transition](transitions.md), and not a modifier, when a part of the build
  must be configured the same way whoever asks for it: a tool always built
  optimized, a library always built for another CPU. A modifier describes what
  the person or the `PACKAGE` file asking for a target wants, and stops
  applying when the target is a dependency of something else.

## Seeing the configuration

`cquery` and `ctargets` print each target with its configuration, and
`audit configurations` lists the constraint values of a configuration:

```sh
$ buck2 ctargets //app:server -m release
root//app:server (cfg:<empty>#952593b9488909b5)

$ buck2 audit configurations 'cfg:<empty>#952593b9488909b5'
cfg:<empty>#952593b9488909b5:
  prelude//cpu/constraints:cpu[x86_64]       (prelude//cpu/constraints:cpu)
  prelude//os/constraints:os[linux]          (prelude//os/constraints:os)
  root//config:build_mode[release]           (root//config:build_mode)
```

`buck2 audit package-values //app` shows the modifiers that `PACKAGE` files
give a package.

## More information

The design is described in the [modifiers RFC](../rfcs/implemented/modifiers.md)
and the
[execution modifiers RFC](../rfcs/implemented/execution_modifiers.md), which
record the design as it was proposed.
