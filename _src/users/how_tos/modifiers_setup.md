---
id: modifiers_setup
title: Setup configuration modifiers
---

Note: this page is not relevant for Meta internal users.

To start using modifiers, all you need to do is call `set_cfg_constructor` from
a `PACKAGE` file.

Start by adding the following to your root `PACKAGE` file:

```python
load("@prelude//cfg/modifier:cfg_constructor.bzl", "cfg_constructor_post_constraint_analysis", "cfg_constructor_pre_constraint_analysis")
load("@prelude//cfg/modifier:common.bzl", "MODIFIER_METADATA_KEY")

native.set_cfg_constructor(
    stage0 = cfg_constructor_pre_constraint_analysis,
    stage1 = cfg_constructor_post_constraint_analysis,
    key = MODIFIER_METADATA_KEY,
    aliases = struct(),
    extra_data = struct(),
)
```

`set_cfg_constructor` is a Buck2 builtin used to setup configuration modifiers.
It supports a few configuration points:

- `stage0` and `stage1` are used to resolve modifiers from the
  [target platform](../../rule_authors/configurations.md) (which is now
  considered deprecated) and the modifiers set in the nearest `PACKAGE` file, on
  the target, and on the CLI. You _can_ override those, but the functions
  provided by the prelude basically do what you would expect.
- `key` is used to fetch modifiers defined on `metadata` attributes. It is only
  supported for legacy reasons, so you should not have to worry about it.
- `aliases` contains modifier aliases to modifier modifiers. Populate it to make
  aliases available from the CLI.
- `extra_data` is used for logging/validation by Meta's internal modifier
  implementation.

As you can see, `aliases` is the only value that one would commonly want to
configure, and it might make sense to call `set_cfg_constructor` with different
`aliases` values in different `PACKAGE` files, if some aliases only make sense
in specific projects (e.g. because they have custom configuration constraints).

The following is an example of exposing custom aliases for build constraints. We
create a `build_mode` constraint with 2 values (`debug` and `release`), which
would most likely be `select`ed in a toolchain definition.

`BUCK`:

```python
constraint_setting(name = "build_mode")

constraint_value(
    name = "debug",
    constraint_setting = ":build_mode",
)

constraint_value(
    name = "release",
    constraint_setting = ":build_mode",
)
```

`PACKAGE`:

```python

load("@prelude//cfg/modifier:cfg_constructor.bzl", "cfg_constructor_post_constraint_analysis", "cfg_constructor_pre_constraint_analysis")
load("@prelude//cfg/modifier:common.bzl", "MODIFIER_METADATA_KEY")

native.set_cfg_constructor(
    stage0 = cfg_constructor_pre_constraint_analysis,
    stage1 = cfg_constructor_post_constraint_analysis,
    key = MODIFIER_METADATA_KEY,
    aliases = struct(
        debug = "//constraints:debug",
        release = "//constraints:release",
    ),
    extra_data = struct(),
)
```

Now, assuming that `:my_target` exists and is affected by the `build_mode`
constraint, we can build it in debug and release mode from the command line
using the `-m`/`--modifier` flag:

```sh
buck2 build :my_target -m debug
buck2 build :my_target -m release
```
