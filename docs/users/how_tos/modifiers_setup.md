---
id: modifiers_setup
title: Set up configuration modifiers
---

import { FbInternalOnly } from 'docusaurus-plugin-internaldocs-fb/internal';

A project has to enable
[configuration modifiers](../../concepts/modifiers.md) before it can use them.
Until it does, Buck2 accepts modifiers and ignores them.

<FbInternalOnly>

Modifiers are already set up in Meta's repositories. This page is for projects
outside Meta.

</FbInternalOnly>

## Enable modifiers

Add this to the [`PACKAGE` file](../../concepts/package_files.md) in the root
of the project, creating the file if there is none:

```python
load(
    "@prelude//cfg/modifier:cfg_constructor.bzl",
    "cfg_constructor_post_constraint_analysis",
    "cfg_constructor_pre_constraint_analysis",
)
load("@prelude//cfg/modifier:common.bzl", "MODIFIER_METADATA_KEY")

native.set_cfg_constructor(
    stage0 = cfg_constructor_pre_constraint_analysis,
    stage1 = cfg_constructor_post_constraint_analysis,
    key = MODIFIER_METADATA_KEY,
    aliases = struct(),
    extra_data = struct(),
)
```

`set_cfg_constructor` tells Buck2 which functions turn a target's
[platform](../../concepts/configurations.md#changing-the-build-configuration)
and modifiers into its configuration:

- `stage0` and `stage1` are those functions. The two from the prelude apply the
  modifiers of `PACKAGE` files, then of the target, then of the command line.
- `key` takes the prelude's `MODIFIER_METADATA_KEY`.
- `aliases` gives short names to modifiers, for use on the command line.
- `extra_data` is passed on to the functions. The prelude's need nothing in it.

`set_cfg_constructor` can be called once, and only in the root `PACKAGE` file
of the project.

## Declare a constraint and aliases for its values

A modifier is a value of a
[constraint](../../concepts/configurations.md#configuration-constraints). This
one, in `config/BUCK`, has the build modes of the project:

```python
constraint(
    name = "build_mode",
    default = "debug",
    values = [
        "debug",
        "release",
    ],
)
```

Its values are the targets `root//config:build_mode[debug]` and
`root//config:build_mode[release]`. To give them the names `debug` and
`release`, list them in the `aliases` of the root `PACKAGE` file:

```python
native.set_cfg_constructor(
    stage0 = cfg_constructor_pre_constraint_analysis,
    stage1 = cfg_constructor_post_constraint_analysis,
    key = MODIFIER_METADATA_KEY,
    aliases = struct(
        debug = "root//config:build_mode[debug]",
        release = "root//config:build_mode[release]",
    ),
    extra_data = struct(),
)
```

An alias can also stand for a list of modifiers.

A constraint changes a build once something `select`s on it. Usually that is a
toolchain that the project declares itself: the demo toolchains that
`buck2 init` sets up have no flags to set. In the `toolchains/BUCK` of
[Setting up a new project](../../getting_started/new_project.md#toolchains),
the C++ toolchain can take its flags from the build mode:

```python
system_cxx_toolchain(
    name = "cxx",
    compiler = "gcc",
    compiler_type = "gcc",
    cxx_compiler = "g++",
    cxx_flags = ["-std=c++20", "-Wall"] + select({
        "root//config:build_mode[debug]": ["-O0", "-g"],
        "root//config:build_mode[release]": ["-O2", "-DNDEBUG"],
    }),
    linker = "g++",
    visibility = ["PUBLIC"],
)
```

## Use it

With a `cxx_binary` named `//app:server`:

```sh
buck2 build //app:server              # debug, the default of the constraint
buck2 build //app:server -m release
buck2 build '//app:server?release'    # the same
```

To check which configuration a target gets:

```sh
$ buck2 ctargets //app:server -m release
root//app:server (cfg:<empty>#952593b9488909b5)

$ buck2 audit configurations 'cfg:<empty>#952593b9488909b5'
cfg:<empty>#952593b9488909b5:
  prelude//cpu/constraints:cpu[x86_64]       (prelude//cpu/constraints:cpu)
  prelude//os/constraints:os[linux]          (prelude//os/constraints:os)
  root//config:build_mode[release]           (root//config:build_mode)
```

Modifiers can also be set
[for a directory](./modifiers_package.md) and
[on a target](./modifiers_target.md).
