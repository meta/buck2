---
id: typechecking
title: Type checking
---

# Python type checking

The Python prelude can type check `python_library`, `python_binary` and
`python_test` targets. Type checking is **opt-in**, and it needs a type
checker to be provided by your Python toolchain: the prelude does not ship
one.

## Turning it on for a target

Set `typing = True` on the target:

```python
python_library(
    name = "lib",
    srcs = ["lib.py"],
    typing = True,
)
```

The related attributes are:

| Attribute                      | Meaning                                                                                                         |
| ------------------------------ | --------------------------------------------------------------------------------------------------------------- |
| `typing`                       | Whether to type check the target. Defaults to `False`.                                                          |
| `typing_validation`            | If `True` (and `typing` is `True`), type errors fail a normal `buck2 build` of the target. Defaults to `False`. |
| `py_version_for_type_checking` | Force the checker to check under a specific Python version.                                                     |
| `shard_typing`                 | Split the check into one action per source file, so files can be checked in parallel and cached independently.  |

## Running the type checker

When the toolchain provides a `type_checker`, each Python target gets a
`[typecheck]` subtarget. Build it to type check that target's sources, with
its dependencies provided as inputs (the dependencies' own `[typecheck]`
subtargets are not built as part of this):

```sh
buck2 build //path/to:lib[typecheck]
```

The output is a JSON file with the checker's results. If `typing` is not
enabled on the target, the subtarget builds successfully and produces an
empty result.

`[typecheck]` also has its own sub-subtargets, which let you check (or
fetch the result of checking) a subset of the sources individually. If
`shard_typing` is **not** enabled (or typing is disabled), there is a single
`[typecheck][shard_default]` covering all sources. If `shard_typing` **is**
enabled, `shard_default` is replaced by one `[typecheck][shard_<path>]`
subtarget per source file instead, with `/` in the path sanitized to `+`
(for example, `[typecheck][shard_foo+bar.py]` for `foo/bar.py`).

To check many targets at once, use the BXL scripts in the prelude:

```sh
# Type check a set of targets or target patterns
buck2 bxl prelude//python/typecheck/batch.bxl:run -- --target //path/to/...

# Type check the targets that own the given files
buck2 bxl prelude//python/typecheck/batch_files.bxl:run -- --source path/to/file.py
```

Pass `--keep-going` to continue past targets that fail to load. (`batch.bxl`
also accepts an `--enable-sharding` flag, but as of this writing it has no
effect — sharding is controlled entirely by the per-target `shard_typing`
attribute.)

## Providing a type checker

The type checker is set with the `type_checker` field of `PythonToolchainInfo`,
which takes a `RunInfo`. Optionally, `typeshed_stubs` (a `ManifestInfo`)
supplies the typeshed stubs that are passed to the checker.

```python
PythonToolchainInfo(
    # ...
    type_checker = ctx.attrs.type_checker[RunInfo],
    typeshed_stubs = ctx.attrs.typeshed_stubs[ManifestInfo],
)
```

You are responsible for the checker itself. Buck2 runs it as:

```
<type_checker> <config.json> --output <result.json>
```

### Input

`config.json` contains:

- `sources`: a list of manifests for the sources being checked
- `dependencies`: a list of manifests for the dependencies
- `typeshed`: the manifest of typeshed stubs, or `null` if the toolchain
  doesn't set `typeshed_stubs`. Adapters that assume a manifest is always
  present will need to handle this case.
- `py_version`: the Python version to check against
- `system_platform`: the platform to check against

### Output

The checker must write a Pyre-compatible JSON object with an `errors` list to
`result.json`. Each error must include a `code`; `name` and `severity` are
optional. Errors that fail validation must also include `path`, `line`,
`column` and `description`.

Diagnostics with code `0`, with the name `unused-ignore` or `unused-type-ignore`,
or with a severity of `info`, `ignore` or `warn` do not fail validation.

The checker must exit successfully whenever it writes a valid result, **even if
that result contains type errors**. A nonzero exit code fails the type-checking
action itself, before Buck2 can turn the result into validation output.

### What is a type checker, and where do typeshed stubs come from?

The `type_checker` is any executable that follows the [Input](#input) and
[Output](#output) contract described above; Buck2 does not ship one.
[Pyrefly](https://pyrefly.org/), Meta's type checker, implements the contract
directly (see below). Other popular checkers, such as
[mypy](https://mypy-lang.org/),
[Pyright](https://microsoft.github.io/pyright/) and
[ty](https://docs.astral.sh/ty/), Astral's type checker, need a small adapter.

Typeshed stubs are `.pyi` files that describe the types of the standard
library and popular third-party packages, separately from their
implementation. A type checker needs them to check code that uses, for
example, `os` or `json`. Most checkers (including `ty`) bundle a copy of
[typeshed](https://github.com/python/typeshed) for the standard library, so
`typeshed_stubs` on the toolchain is only needed if you want to supply your
own (for example, pinned stubs for third-party packages, or a hermetic build
that must not rely on whatever the checker happened to bundle).

### Worked example: Pyrefly

Pyrefly's `buck-check` command takes the `config.json` described above and
writes the `{"errors": [...]}` result, and it exits successfully even when it
finds type errors. So no adapter is needed: point `type_checker` at it
directly.

```python
PythonToolchainInfo(
    # ...
    type_checker = RunInfo(args = ["pyrefly", "buck-check"]),
)
```

Buck2 appends `<config.json> --output <result.json>`, which is what
`pyrefly buck-check <INPUT_PATH> --output <FILE>` expects. This was tested by
running `pyrefly buck-check` (version 1.3.2) by hand on a `config.json` in the
shape Buck2 writes: a file with a type error produced one error with `line`,
`column`, `path`, `code`, `name`, `description` and `severity`; a clean file and
an empty `sources` list both produced `{"errors": []}`; and the exit code was
`0` in all three cases. It was not run inside a full Buck2 build.

Pyrefly also has a separate integration for editors and for running
`pyrefly check` directly in a Buck2 repository, configured with
`[build-system] type = "buck"` in `pyrefly.toml`. See the
[Pyrefly configuration docs](https://pyrefly.org/en/docs/configuration/#buck2),
which note that build system support is currently unstable.
