---
id: how_to_handle_errors
title: How to Handle Errors
---

This guide shows you how to handle failures in BXL script when running bxl
operations, like analysis, queries, build artifact and other operations. You can
find all available operations at [here](../../../api/bxl/LazyContext). You'll
learn how to recover from errors and running these operations in parallel.

## Running Single Operations with Error Recovery

To run a single operation with error handling:

### 1. Create a lazy operation using the appropriate

[ctx.lazy](../../../api/bxl/LazyContext) method:

```python
lazy_analysis = ctx.lazy.analysis(node)
```

It will return a [`Lazy`](../../../api/bxl/Lazy) object.

### 2. Add error handling and resolve the operation:

```python
result = lazy_analysis.catch().resolve()
```

The result will be of type
[`Result[bxl.AnalysisResult]`](../../../api/bxl/Result), allowing you to check
for and handle errors.

```python
if result.is_ok():
    analysis_res = result.unwrap()
else:
    error = result.unwrap_err()
```

If resolving [`Lazy`](../../../api/bxl/Lazy) object without calling `catch()`,
it will return `bxl.AnalysisResult` and the bxl script will fail if this
operation fails.

## Handling Multiple Operations in Parallel

To run multiple operations while handling potential failures:

### 1. Create your lazy operations:

```python
lazy_ops = [
    ctx.lazy.analysis(node1),
    ctx.lazy.analysis(node2),
    ctx.lazy.configured_target_node(target1)
]
```

### 2. Choose your error handling approach:

#### For collective error handling (stop on first error):

```python
result = ctx.lazy.join_all(lazy_ops).catch().resolve()
```

The Return type is `Result[list[bxl.AnalysisResult]]`

#### For individual error handling per operation:

```python
result = ctx.lazy.join_all([op.catch() for op in lazy_ops]).resolve()
```

The Return type is `list[Result[bxl.AnalysisResult]]`

## Examples

Examples can be found at
`tests/core/bxl/test_lazy_build_artifact_data/lazy_build_artifact.bxl` of buck2
repo folder.

Run such command at `tests/core/bxl/test_lazy_build_artifact_data` to run the
example bxl script

```sh
buck2 bxl lazy_build_artifact.bxl:build_artifact
```
