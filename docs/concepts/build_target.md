---
id: build_target
title: Build Target
---

# Build Target

A _build target_ is a string that identifies a build target in your project.
Build targets are used as arguments to Buck2 commands, such as
[`buck2 build`](../../users/commands/build) and
[`buck2 run`](../../users/commands/run). Build targets are also used as
arguments to [build rules](build_rule.md) to enable one target to reference
another. For example, a build rule might use a build target to reference another
target in order to specify that target as a _dependency_.

#### Fully-qualified build targets

Here is an example of a _fully-qualified_ build target:

```
cell//java/com/facebook/share:ui
```

A fully-qualified build target has three components:

1. The `cell//` prefix indicates that the subsequent path is from the _root_ of
   `cell`.
2. The `java/com/facebook/share` between the `//` prefix and the colon (`:`)
   indicates that the [build file](build_file.md) (usually named `BUCK`) is
   located in the directory `java/com/facebook/share`.
3. The `ui` after the colon (`:`) indicates the name of the build target within
   the build file. Build target names must be unique within a build file. By
   _name_ we mean, more formally, the value of the `name` argument to the build
   rule.

Note that the name of the build file itself—usually BUCK—does _not_ occur in the
build target. A package has exactly one build file, so it is unnecessary to
include the name in the target. The names that Buck2 accepts for build files
are `BUCK.v2` and `BUCK` by default, and each cell can change them in the
`[buildfile]` section of its `.buckconfig`. A fully-qualified build target has
this form, in which the last part shows the characters that a target name can
have:

```
[A-Za-z0-9._-]*//[A-Za-z0-9/._-]*:[A-Za-z0-9_/.=,@~+!$\\-]+
|- cell name -|  | package path | |----- target name ------|
```

In Buck2, a _cell_ defines a directory tree of one or more Buck2 packages. For
more information about Buck2 cells and their relationship to packages and
projects, see the [Key Concepts](key_concepts.md) topic. **NOTE:** A target
path that starts with a double forward slash (`//`) is relative to the root of
the current cell. On the command line, a target path without the `//` is
relative to the current directory (see **Pro Tips** below). In a build file,
the only _relative_ build targets are the ones that start with a colon. See
**Package relative build targets** below for more details.

#### Cell relative build targets

A _cell relative_ build target omits the cell, and is inferred to be relative to
the current cell.

#### Package relative build targets

A _package relative_ build target can be used to reference a build target
_within the same _[_build file_](build_file.md) (aka _package_). A relative
build target starts with a colon (`:`) and is followed by only the third
component (or _short name_) of the fully-qualified build target. The following
snippet from a build file shows an example of using a relative path.

```python
## Assume this target is in //java/com/facebook/share/BUCK#
java_binary(
  name = 'ui_jar',
  deps = [
    ## The following target path
    ##   //java/com/facebook/share:ui
    ## is the same as using the following relative path.#
    ':ui',
  ],
)
```

## Command-line Pro Tips

Here are some ways that you can reduce your typing when you specify build
targets as command-line arguments to the `buck2 build` or `buck2 run` commands.
Consider the following example of a fully-qualified build target used with the
`buck2 build` command:

```sh
buck2 build cell//java/com/facebook/share:share
```

Although Buck2 is always strict when parsing build targets in build files, Buck2
is flexible when parsing build targets on the command-line. Specifically, a
target without the leading `//` is relative to the current directory, so from
the root of `cell` the above could be:

```sh
buck2 build java/com/facebook/share:share
```

Also, if there is a forward slash before the colon, it is ignored, so this could
also be written as:

```sh
buck2 build java/com/facebook/share/:share
```

which is the form that tab-completion of the directory produces.

Finally, if the final path element matches the value specified after the colon,
it can be omitted:

```sh
# This is treated as //java/com/facebook/share:share.
buck2 build java/com/facebook/share/
```

which makes the build target even easier to tab-complete. For this reason, the
name of the build target for the primary deliverable in a build file is often
named the same as the parent directory. That way, it can be built from the
command-line with less typing.

## See also

Buck2 supports the ability to define **_aliases_ for build targets**; using
aliases can improve brevity when specifying targets on the Buck2 command line.
For more information, see the [`[alias]`](buckconfig.md#alias) section in the
documentation for [`.buckconfig`](buckconfig.md). A
[**build target pattern**](target_pattern.md) is a string that describes a set
of one or more build targets. For example, the pattern `//...` is used to build
an entire project. For more information, see the **Build Target Pattern** topic.
