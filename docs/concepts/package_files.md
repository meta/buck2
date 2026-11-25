---
id: package_files
title: PACKAGE Files
---

A `PACKAGE` file sets defaults for every target in its directory and in the
directories below it. A project uses `PACKAGE` files to say, once, things that
would otherwise be repeated in every `BUCK` file of a directory tree:

- which targets may depend on the targets there (their default
  [visibility](visibility.md));
- how the targets there are built (their
  [configuration modifiers](modifiers.md));
- values of the project's own that its macros read.

`PACKAGE` files are optional. A project can have none, one in its root, or one
in any number of directories.

:::note

Despite the name, a `PACKAGE` file does not make a
[package](key_concepts.md#packages). A package is a directory with a
[build file](build_file.md), and a directory with only a `PACKAGE` file has no
targets. A `PACKAGE` file applies to all the packages at and below its
directory.

:::

## An example

With this `PACKAGE` file in `services/`:

```python
load("@prelude//cfg/modifier:set_cfg_modifiers.bzl", "set_cfg_modifiers")

# Targets under services/ can be used from anywhere in the project.
package(visibility = ["PUBLIC"])

# Targets under services/ are built in release mode.
set_cfg_modifiers(["root//config:build_mode[release]"])
```

a target in `services/BUCK` or `services/auth/BUCK` that has no `visibility` of
its own is visible to every target. When a command names one of these targets,
the target is built with the `release` value of the `build_mode`
[constraint](configurations.md#configuration-constraints), unless the target or
the command line asks for another. The modifier has an effect once the project
has [enabled modifiers](../users/how_tos/modifiers_setup.md).

## How PACKAGE files are evaluated

Before Buck2 evaluates a `BUCK` file, it evaluates the `PACKAGE` file in the
same directory and in each directory above it, starting from the project root.
What an outer `PACKAGE` file sets is in effect in the directories below, where
an inner `PACKAGE` file can add to it or replace it.

- Directories are followed up to the project root, whichever
  [cell](key_concepts.md#cells) they are in. A `PACKAGE` file in the project
  root applies to `toolchains//` and to `prelude//` as well, so a `within_view`
  there also limits what the prelude's own targets may depend on.
- A `PACKAGE` file is a Starlark file. It can `load()` `.bzl` files, and read
  `.buckconfig` values with `read_config`. It cannot declare targets: rules such
  as `genrule` are not defined in a `PACKAGE` file.
- Changing a `PACKAGE` file re-evaluates the `BUCK` files it applies to.
- The file can also be named `BUCK_TREE`. Use that name in a directory that
  holds a subdirectory named `package`, which on a case-insensitive filesystem
  cannot exist next to a file named `PACKAGE`.

## What a PACKAGE file can do

### Set default visibility

```python
package(
    visibility = ["//services/..."],
    within_view = ["//services/...", "//third-party/..."],
)
```

`visibility` is the visibility of every target below that does not set its own.
`within_view` limits what the targets below may depend on. See
[Visibility](visibility.md) for what the patterns mean.

A `package()` call replaces both lists as the `PACKAGE` files above set them,
including a list that the call leaves out. With `inherit = True`, it adds to
them instead.

### Set configuration modifiers

`set_cfg_modifiers`, from the prelude, adds [modifiers](modifiers.md) to every
target below, as in the example above. See
[Add configuration modifiers to entire directories](../users/how_tos/modifiers_package.md).

The `PACKAGE` file in the project root is also where modifiers are enabled for
the project, with `native.set_cfg_constructor`. That function can only be called
there. See
[Set up configuration modifiers](../users/how_tos/modifiers_setup.md).

### Carry values for macros

A `PACKAGE` file can store a value under a name:

```python
write_package_value("myproject.team", "payments")
```

and a macro called from a `BUCK` file below can read it:

```python
def team():
    return read_package_value("myproject.team")
```

The name must have exactly one `.` in it. A `PACKAGE` file further down can read
the value it inherits with `read_parent_package_value`, and replace it by
writing the name again with `overwrite = True`.

## Seeing what applies to a directory

```sh
buck2 audit package-values //services/auth
```

prints the visibility, the modifiers and the values that the `PACKAGE` files
give the package `//services/auth`.

## Reference

[PACKAGE File API](../rule_authors/package.md) describes `package()` and the
functions for package values.
