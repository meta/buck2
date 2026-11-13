# Package-local values

This RFC proposes to extend buck2 Starlark with package-local values.

## Why

DevX people want to have some per-directory configuration files, accessible from
Starlark macros.

For example, a project NNN may want to switch to building using LLVM 15 by
default. End users would want to have an easy instruction how to do that, after
DevX people provided instructions and infrastructure for that.

## What we have now

Currently, in fbcode, we have `get_modes` mechanism.

`get_modes` symbol is registered in per-package implicit symbols,
[here](https://fburl.com/code/7ud7e3ci).

This symbol can be accessed from macros using
[implicit_package_symbol](https://fburl.com/code/u5coj9s7) function.

`get_modes` functions are package-local, but all `BUILD_MODE.bzl` files need to
be registered in global buckconfig, which is not ideal.

Proposed per-package properties can replace `get_modes` mechanism.

## API

### `PACKAGE` files

Before evaluating `BUCK` file, buck2 will evaluate all `PACKAGE` files in the
same directory and all parent directories. Absent `PACKAGE` files are treated as
empty files.

All relevant `PACKAGE` files are executed sequentially from the root directory
to the current directory (but unrelated `PACKAGE` files can be executed in
parallel). Evaluating `PACKAGE` files sequentially provides additional
guarantees, for example, attempt to override a property (unless explicitly
requested) should fail with Starlark call stack.

Each `PACKAGE` file is evaluated at most once (like `bzl` file).

`PACKAGE` files may load arbitrary `bzl` files. `BUCK`-specific functions called
in `bzl` files (like rule functions) are available, but calling functions from
`PACKAGE` files is an error. This way, `bzl` files are evaluated only once
regardless of whether they are loaded from `PACKAGE` or `BUCK` file.

### API

`PACKAGE` files have a global function:

#### `PACKAGE` file API

```python
def write_package_value(
    name: str,
    value: "",
    overwrite: bool = False,
): ...
```

Name is a string which must contain exactly one dot symbol (just to enforce code
style).

Value is an arbitrary Starlark value, for example, an integer, a list of
integer, a struct or a function.

When `overwrite` is `False` (default), attempt to overwrite per-package value
defined in parent `PACKAGE` file will fail.

Written values are frozen when `PACKAGE` file evaluation is finished.

Note `write_package_value` symbol exists in `bzl` globals, and it can be called
from `bzl` file in context of `PACKAGE` evaluation, but calling
`write_package_file` is an error on context of `BUCK` evaluation.

Modifying `PACKAGE` file logically invalidates the `BUCK` file of this package,
and all `PACKAGE` and `BUCK` files of subpackages. However, `BUCK` file
evaluation may track which package-local values were accessed and only
invalidate `BUCK` files which were potentially affected (similarly to how we do
it with buckconfigs, with individual properties being projection keys).

#### `BUCK` file API

`BUCK` files (and `bzl` files included from `BUCK` files) have a global
function:

```python
def read_package_value(
    name: str,
): ...
```

This function returns the nearest value registered per package, or `None` is
such value does not exist.

This function is available in `bzl` files, but attempt to call this function in
context of `PACKAGE` file evaluation results in an error. This restriction can
be lifted in the future.

Per-package values are **not** accessible as global symbols in `BUCK` files. We
may reconsider it in the future.

### `read_config`

`PACKAGE` files may call `read_config` function.
