---
id: load
title: load()
---

The `load()` function is used to include definitions (functions, macros, rules,
and constants) from another `.bzl` file.

In Buck2, `load()` executes a `.bzl` file and imports the specified symbols into
the current file's namespace. This allows you to share code across multiple
build files and create reusable build components.

## Overview

The primary use cases for `load()` include:

- **Sharing constants** - Common configuration values used across multiple
  targets
- **Defining macros** - Functions that generate one or more build rules
- **Importing rules** - Custom rule definitions for specific build scenarios
- **Utility functions** - Helper functions for build logic

The `load()` function can be used in:

- `BUCK` files (build files)
- `PACKAGE` files (package configuration)
- Other `.bzl` files (creating libraries of build code)

## Syntax

```python
load("//path/to/file:name.bzl", "symbol1", "symbol2", ...)
```

Or with aliases:

```python
load("//path/to/file:name.bzl", alias1="symbol1", alias2="symbol2")
```

## Arguments

- **Path** (required): The first argument is a string specifying the label of
  the `.bzl` file to load. This is similar to Buck2's target label syntax but diverges in the `cell` syntax with a prefixed `@`:
  - Absolute to cell: `//package/path:file.bzl`
  - Relative to package: `:file.bzl`
  - From external cell: `@cell//package/path:file.bzl`

- **Symbols** (required): One or more symbols to import from the specified file.
  These can be:
  - **Positional arguments** - Import symbols with their original names:
    `"symbol1", "symbol2"`
  - **Keyword arguments** - Import symbols with aliased names:
    `local_name="remote_name"`

## Examples

### Loading Constants

Suppose the file `//core:defs.bzl` contains:

```python
COMPILER_FLAGS = [
    "-Wall",
    "-Werror",
    "-O2",
]
```

You can load these constants in a `BUCK` file:

```python
load("//core:defs.bzl", "COMPILER_FLAGS")

cxx_binary(
    name = "example",
    srcs = ["main.cpp"],
    compiler_flags = COMPILER_FLAGS,
)
```

### Loading with Aliases

To avoid name collisions, you can rename symbols during import:

```python
load("//core:defs.bzl", my_flags="COMPILER_FLAGS")

cxx_binary(
    name = "example",
    srcs = ["main.cpp"],
    compiler_flags = my_flags,
)
```

### Loading Macros

If `//build_defs:macros.bzl` contains:

```python
def custom_library(name, srcs, **kwargs):
    cxx_library(
        name = name,
        srcs = srcs,
        compiler_flags = ["-std=c++17"],
        **kwargs
    )
```

You can load and use the macro:

```python
load("//build_defs:macros.bzl", "custom_library")

custom_library(
    name = "mylib",
    srcs = ["lib.cpp"],
    deps = ["//other:dep"],
)
```

### Loading Custom Rules

```python
load("//rules:custom.bzl", "my_custom_rule")

my_custom_rule(
    name = "custom_target",
    src = "input.txt",
)
```

### Multiple Symbols

You can load multiple symbols in a single statement:

```python
load(
    "//common:defs.bzl",
    "COMMON_FLAGS",
    "helper_function",
    local_rule="exported_rule",
)
```

## Best Practices

### Naming

- Use `UPPER_CASE` for constants
- Use `snake_case` for functions and macros
- Use descriptive names that indicate purpose

### Organization

- `load()` statements must be top level statements and cannot be conditionally executed
- `load()` statements should be kept at the top of files for clarity

### Visibility

- `.bzl` files are automatically visible to any target that can see the package
  they're in
- Use [package visibility](./package.md) to control access to `.bzl` files
- Keep `load()` statements at the top of files for clarity

### Performance

- Avoid expensive computation in `.bzl` files during load time
- `.bzl` files are evaluated once and cached, so minimize side effects

## Common Patterns

### Conditional Loading

While you cannot conditionally execute `load()` statements (they must be at the
top level), you can conditionally use the loaded symbols:

```python
load("//config:settings.bzl", "ENABLE_FEATURE")

def maybe_add_feature(name, srcs):
    if ENABLE_FEATURE:
        feature_library(name=name, srcs=srcs)
    else:
        regular_library(name=name, srcs=srcs)
```

### Re-exporting

You can create a `.bzl` file that re-exports symbols from other files:

```python
# //defs:all.bzl
load("//defs:cpp.bzl", "cpp_macro")
load("//defs:python.bzl", "py_macro")

# Re-export for convenience
export_cpp = cpp_macro
export_py = py_macro
```

## See Also

- [Writing Rules](writing_rules.md) - How to create custom build rules
- [PACKAGE Files](package.md) - Package-level configuration
- [Build APIs](../api/build/index.md) - Available Starlark functions and types
