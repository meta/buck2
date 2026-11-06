---
id: how_to_cache_and_share_operations
title: How to Cache and Share Operations
---

This guide shows you how to use anonymous targets in BXL to cache and share
operations across different commands, improving bxl performance and reducing
peak memory usage.

## When to use caching

Use anonymous target caching if you need to:

1. Cache results of expensive Starlark computations
2. Share cached work across different parts of your bxl
3. Share cached work across different bxl commands
4. Reduce peak memory usage

## Basic caching with anonymous targets

### 1. Define bxl anon target rule

```python
my_anon = bxl.anon_rule(
    impl = _my_anon_impl,
    attrs = {
        "foo": attrs.int(),
        "bar": attrs.str(),
        ...
    },
)
```

You can find the supported attributes at the "Attributes" sections
[here](../../../rule_authors/anon_targets/)

### 2. Define the anon target impl

```python
def _my_anon_impl(bxl_ctx: bxl.Context, attrs: struct) -> list[Provider]
    # Your implementation here
    return [DefaultInfo(...), ...]
```

### 3. Create and resolve the anonymous target in your BXL script

You can use
[`actions.anon_target`](../../../api/build/AnalysisActions/#analysisactionsanon_target)
to create one anon target or
[`actions.anon_targets`](../../../api/build/AnalysisActions/#analysisactionsanon_targets)
to create several anon targets.

```python
def _bxl_main_impl(bxl_ctx: bxl.Context):
    ...
    actions = ctx.bxl_actions().actions

    # Create anonymous target
     promise = actions.anon_target(
        my_anon,
        attrs = {
            "foo": 42,
            "bar": "hello world",
        }
    ).promise

    # Resolve the anon target result
    result = ctx.resolve(actions, promise)

    # Use the anon target result
    ...
```

Now you have an anon target and the output of this anon target will be cached
using a cache key composed of its attributes, target platform, and any bxl
script modifiers.

## Examples

Examples can be found at 'tests/core/bxl/test_anon_bxl_data/anon_bxl.bxl' of
buck2 repro folder.

Run such command at `tests/core/bxl/test_anon_bxl_data` to run the example bxl
script

```sh
buck2 bxl anon_bxl.bxl:eval_anon_bxl
```

## Further Reading

You can refer [Anonymous Targets](../../../rule_authors/anon_targets/) to learn
more about it.
