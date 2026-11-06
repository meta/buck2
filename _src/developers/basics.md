---
oncalls: ['build_infra']
---

# Buck2 Developer Basics

This file is `docs/developers/basics.md`. It is required reading for working on buck2 itself.

This file is optimized for both humans and LLMs and must be kept short. Detailed explanations belong
in adjacent files in this directory or other documentation from which humans or LLMs can pull as
needed.

## Validation

buck2 is built with buck2 internally at Meta. Cargo builds are primarily for OSS. To validate
changes:

```bash
# Check that things compile
# Required for LLMs making changes to `buck2/app`
arc rust-check fbcode//buck2/app/...
# Run clippy
arc rust-clippy fbcode//buck2/app/...
# Run lints and apply fixes
arc lint -a
# Format code. Usually unnecessary, performed by IDEs and hooks
arc f
```

In open source, `python3 fmt.py` formats Rust and Starlark exactly as `arc f` does, and the
"Format" GitHub check enforces it; `python3 fmt.py --check` only reports.

Buck2 has standard Rust unittests and integration tests at `tests/core`.

```bash
# Run an integration test
buck2 test fbcode//buck2/tests/core/analysis:test_cmd_args
# Discover more information about writing and executing integration tests
cat tests/core/README.md
# Run some unittests
buck2 test fbcode//buck2/app/buck2_core:buck2_core
```

In OSS, standard cargo tooling applies. Integration tests do not run in OSS.

## Coding conventions

Most important of all: Most questions can be answered by matching the conventions and style
of nearby code.

Standard `rustfmt` conventions apply. Beyond that:

- **Maps and sets**: use the `buck2_hash` aliases; its crate docs say which one fits a
  given situation. Naming `std`, `indexmap`, `dashmap` or `starlark_map` collections directly
  is lint-enforced in `app/` and `buck2_tpx/`.
- **Cloning**: prefer `.dupe()` over `.clone()` for types that implement `Dupe`
  (e.g. `Arc`-wrapped types). Use `gazebo` utilities — particularly `dupe` —
  where they fit.
- **String conversion**: prefer `.to_owned()` over `.to_string()` for `&str` →
  `String`.
- **Durations**: never call `.elapsed()` (lint-enforced in `app/`); compute
  `Instant::now() - start` so the time source is explicit.
- **Imports**: use `use crate::foo::bar`, not `use super::bar`. Place all `use`
  statements at the module level — never inside a function or block. Test
  modules may use `use super::*;` at the top.
- **Modules**: a module should contain either submodules OR types/functions, not
  both.
- **PartialEq/Hash with ignored fields**: use the `derivative` crate.

### Error message style

- Names (variables, targets, files, ...) should be quoted with backticks, e.g.
  ``Variable `x` not defined``.
- Lists should use square brackets, e.g. ``Available targets: [`aa`, `bb`]``.
- Error messages should start with an upper case letter and should not end with
  a period.

## Error handling

Buck2 uses `buck2_error` replacing both `anyhow` and `thiserror`. The must-knows:

- Return `buck2_error::Result<T>`.
- Define error types with `#[derive(Debug, buck2_error::Error)]` and tag them
  with `#[buck2(tag = ...)]` (no `thiserror::Error`).
- Use the `buck2_error!` macro for ad-hoc errors.
- `.expect()`, `.unwrap()`, etc. are ok for file-local invariant violations/"this should never
  happen" cases. If not file-local, prefer `internal_error!()`, `.internal_error("...")?` or
  `.with_internal_error(|| ...)` if possible.
- Inspecting or creating `buck2_error::Error`s in non-error codepaths is strongly discouraged.
  Represent states that are not errors using types that are not errors or at least dedicated,
  semantically clear error types.

For more details including about defining errors, tagging, conversion, and context see [Error
Handling](./error_handling.md).

## Feature gates

Gate new or risky behavior with buckconfig, not environment variables: a
`[buck2]`-section key, read where DICE can track it (grep for
`BuckconfigKeyRef` with `section: "buck2"` for the pattern), and named so
the value flips false -> true as the feature rolls out. Use
`RolloutPercentage` in place of `bool` when you want hostname-hashed
percentage rollout. Reserve `buck2_env!` for the few places configuration
cannot reach: the client before it talks to the daemon, and test-only
knobs.

## Internal vs open source

Code is generally the same internally and externally, exceptions will be locally self-explanatory.

## Rust Dependencies

When modifying dependencies internally at Meta, change BUCK files. Our Cargo.toml files are
maintained by autocargo, run `arc autocargo -p buck2` to update them. See
./cargo_internally.fb.md for more.

Autocargo is not available outside Meta. Hand-edit Cargo files if needed, we will deal with it on
import.

At Meta, common third party Rust libraries are generally just available.

# Debugging

```bash
# Build buck2
buck2 build @fbcode//mode/opt fbcode//buck2:buck2 --out /tmp/buck2_dest
# Build buck2 from source and run a command with it in a different isolation dir
./buck2.py build :foo
```

Further information at [debugging.md](./debugging.md)

# Perf work

For details on profiling and benchmarking, see [perf/basics.md](./perf/basics.md).
