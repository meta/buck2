---
id: bxl_cquery_vs_cli_cquery
title: BXL cquery vs. Buck2 CLI cquery - Divergence in Configuration Handling
---

## Overview

The command `buck2 cquery` and function `cquery` in bxl share the same goal but
sometimes yield different results. This document explains why these commands
might produce divergent results for identical-looking queries and how their
underlying mechanisms diverge. By contrasting their approaches to configuration
universes, we clarify how to align BXL scripts with CLI behavior for consistent
results.

## A Concrete Example

Imagine we have:

- A target `X` with a default configuration `cfg_a`.
- Targets `A`, `B`, and `C` in `root//path/...` with a default configuration
  `cfg_b`.
- Both `A` and `C` directly depend on `X`, but each dependency is configured as
  `cfg_b`.

Assume that our goal is to determine the direct reverse dependencies of `X`
within `root//path/...`. Let's look at two approaches:

### `buck2 cquery`

We can do a query like this:

```shell
buck2 cquery 'rdeps(root//path/..., X, 1)`
```

It will return `A (cfg_b)`, `C (cfg_b)` and `X(cfg_b)`

### `cquery` in bxl

```python

def _main(ctx):
    res = ctx.cquery().rdeps("root//path/...", "X", depth = 1)
    ctx.output.print(res)

main = bxl_main(
    cli_args = {},
    impl = _main
)

```

It will return empty.

## Core Difference

### CLI `buck2 cquery`

When no [target universe](../../concepts/glossary.md#target-universe) is
provided via the `--target-universe` CLI argument, it constructs the
[target universe](../../concepts/glossary.md#target-universe) though the
following process:

#### Phase 1: Target Resolution

1. **Pattern Resolution**: Resolve target patterns (`root//path/...` and `X`) to
   obtain unconfigured targets: `A`, `B`, `C`, and `X`.

2. **Target Configuration**: With no additional configuration arguments, the
   default target platform is applied. This produces configured targets:
   - `A (cfg_b)`
   - `B (cfg_b)`
   - `C (cfg_b)`
   - `X (cfg_a)`

3. **Universe Construction**: Build the target universe using these configured
   targets. Since `A (cfg_b)` and `C (cfg_b)` depend on `X (cfg_b)`, the
   universe includes:

   ```
   A (cfg_b)
   B (cfg_b)
   C (cfg_b)
   X (cfg_a)
   X (cfg_b)
   ```

4. **Resolve the literal in the universe**: Lookup the original patterns within
   the constructed universe:
   - `root//path/...` resolves to `A (cfg_b)`, `B (cfg_b)`, `C (cfg_b)`
   - `X` resolves to both configurations: `X (cfg_a)` and `X (cfg_b)`

#### Phase 2: run rdeps function

It will go though `rdeps` logic:

Within the universe of targets under `root//path/...` (i.e., `A (cfg_b)`,
`B (cfg_b)`, and `C (cfg_b)`), identify the targets whose direct dependencies
include `X (cfg_a)` or `X (cfg_b)`. In this case, those targets are `A (cfg_b)`
and `C (cfg_b)`.

Furthermore, if `X (cfg_a)` or `X (cfg_b)` itself exists within the universe of
`A (cfg_b)`, `B (cfg_b)`, and `C (cfg_b)`, it should also be included in the
results. In this case, `X (cfg_b)` is part of the universe, so it is included.

Thus, the final result is `A (cfg_b)`, `C (cfg_b)`, and `X (cfg_b)`.

### `cquery` in bxl

The bxl logic differs from the `buck2 cquery` approach.

#### Phase 1: resolve arguments to configured target nodes if needed

If the arguments are not configured target(s), apply the default target platform
(if not given) to do the configurations. So in this case:

- The first argument resolves to `A (cfg_b)`, `B (cfg_b)` and `C (cfg_b)`
- The second argument resolves to `X (cfg_a)`.

#### Phase 2: run rdeps function

This phase follows the same logic of "Phase 2" in CLI `buck2 cquery`

But in this case, the second argument is different, it only has `X (cfg_a)`.
Since `A (cfg_b)`, `B (cfg_b)` and `C (cfg_b)` do not directly depend on
`X (cfg_a)`, the result will be empty here.

## Aligning BXL with CLI Behavior

To replicate the same results in BXL, you should provide a universe explicitly:

1. Create a universe of all arguments

```python
universe = ctx.target_universe(["root//path/...", "X"])
```

2. Lookup args in the universe

```python
arg0 = universe.lookup("root//path/...")
arg1 = universe.lookup("X")
```

3. Run cquery

```python
res = ctx.cquery().rdeps(arg0, arg1, depth=1)
```

Alternatively, you can also achieve the same result with a single command:

```python
res = ctx.cquery().eval('rdeps(root//path/..., X, 1)')
```

`ctx.cquery().eval()` will do same logic when we do in CLI

## Reflection: Philosophy of Configuration Management in CLI and cquery

The divergence between CLI and BXL cquery reflects a broader design trade-off:

- CLI: Optimized for user-friendliness, abstracting configuration logic.
- BXL: Prioritizes flexibility for advanced use cases, requiring explicit
  control (such as ability to provide target universes).
