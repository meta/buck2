---
id: how_to_catch_building_artifacts_errors
title: How to Catch Building Artifacts Errors
---

This guide shows you how to properly handle artifact building errors in BXL
using the
[`ctx.lazy.build_artifact`](../../../api/bxl/LazyContext/#lazycontextbuild_artifact)
API.

## Prerequisites

Read [How to Handle Errors](../how_to_handle_errors) first to first to
understand BXL's error handling patterns.

## Best Practices

### 1. Prepare the artifacts to be built

### 2. Use `ctx.lazy.build_artifact` api

```python
lazy_built = ctx.lazy.build_artifact(artifact)
# catch error and resolve Lazy object
result = lazy_built.catch().resolve()
```

For how to parallel building a list of artifacts please refer
[here](../how_to_handle_errors/#handling-multiple-operations-in-parallel) for
more details

### 3. Call `ctx.output.ensure/ensure_multiple` to materialize artifacts

```python
if result.is_ok():
    artifact = result.unwrap()
    ctx.output.ensure(artifact)
else:
    error = result.unwrap_err()
    print(error)
```

## Important Limitations

You cannot use this API for artifacts declared in BXL.

## Examples

Examples can be found at
`tests/core/bxl/test_lazy_build_artifact_data/lazy_build_artifact.bxl` of buck2
repo folder.

Run such command at `tests/core/bxl/test_lazy_build_artifact_data` to run the
example bxl script

```sh
buck2 bxl lazy_build_artifact.bxl:build_artifact
buck2 bxl lazy_build_artifact.bxl:build_artifact_fail
```
