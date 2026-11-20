# RFC: labels -> metadata attribute

This RFC proposes to add new builtin per target attribute: `metadata`, as
replacement for `labels`.

## Context: labels

In buck1 we have `labels` builtin rule attribute, which is a list of strings.

In buck2 we have `labels` attribute which is configured in prelude, it does not
have special meaning.

## Context: package values

`PACKAGE` files have a function: `write_package_value(key, value)`, where a key
is a word-dot-word string, and value is arbitrary starlark value which should be
serializable as JSON.

## Context: metadata we use or we need

There are several spaces where we use or need metadata to be stored in buck2
target graph.

- fbcode uses per-package values to switch code to new clang
  ([example](https://www.internalfb.com/code/fbsource/[ef740e6f2610c64621f7547a3b46d54d32af8600]/fbcode/ownership/code_metadata/PACKAGE?lines=3))
- testinfra wants to use `PACKAGE` values to mark a set of folders to a logical
  larger project
- it is likely that per-target `metadata` attribute should be used in
  configuration factory function.
- TD wants to declare CI trigger jobs per-target or per-package, and this logic
  is to be specified in `BUCK` or `PACKAGE` files — as metadata

## Proposal: metadata attribute

Add builtin `metadata` attribute to all the targets.

`metadata` has the same structure as package values: word-dot-word to arbitrary
value serializable to JSON.

For example:

```python
cxx_library(
    name = "mylib",
    metadata = {
        "td.run_on_windows": True,
    },
)
```

Metadata attribute is not configurable (means `select` cannot be used).
