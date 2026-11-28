# APIs

A lot of Buck2 is driven by Starlark APIs. While there is a
[Starlark specification](https://github.com/bazelbuild/starlark/blob/master/spec.md),
for most purposes it can be considered a subset of Python. There are four main
places you can write Starlark in Buck2:

- In `BUCK` files, where you declare targets by calling rules. The most
  interesting functions are [the rules themselves](../prelude/rules/), but you
  will often use the [builtin Starlark functions](starlark) (most of which are
  the same as in Python), and a few of the [build functions](build) (e.g.
  `glob`).
- In `.bzl` files, where you define rules and macros. You can use the same
  Starlark standard functions, but will heavily be using the
  [build functions](build) (e.g. `rule` and `attrs`).
- In [`PACKAGE` files](concepts/package_files.md), which set defaults for the
  targets of a directory tree.
- In [BXL](../bxl), where the [context type](bxl/Context) is one of the more
  important ones.
