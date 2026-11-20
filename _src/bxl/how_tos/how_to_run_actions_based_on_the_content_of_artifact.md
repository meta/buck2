---
id: how_to_run_actions_based_on_the_content_of_artifact
title: How to run actions based on the content of artifact
---

This guide shows you how to run actions that need to read artifact contents
first. You'll learn how to use
[dynamic actions](../../../rule_authors/dynamic_dependencies) to handle cases
where you need to read a file's content first.

Common examples include:

- Reading a dependency list file to determine compilation order
- Processing an index file to find required dependencies
- Parsing source files to discover header dependencies
- Reading configuration files to determine build parameters

## Steps

### 1. Identify and prepare your dynamic dependencies

First, determine which artifacts you need to read before running your action.
These will be your "dynamic" artifacts. For example:

- A dependency file listing required inputs
- A configuration file specifying build parameters
- A index file listing the dependencies

You need to either use BXL APIs to obtain your artifacts from the build graph,
or run actions to generate them.

### 2. Declare your output artifacts

Declare the outputs that your dynamic action will produce:

```python
output_artifact = ctx.actions.declare_output("output.txt")
```

### 3. Define dynamic action

```python
process_dynamic = bxl.dynamic_actions(
    impl = process_dynamic_impl,
    attrs = {
        "file": dynattrs.artifact_value(),
        "output": dynattrs.output(),
        "my_data": dynattrs.value(MyData)
    }
)
```

It needs a impl function which we will define in step 4, and attributes that you
want to passed in the dynamic actions includes the artifacts you want to read,
output artifacts you declared in step 2, and any other values you want to pass
in. You can using `dynattrs.value([type])` to pass in any type. More details for
dynamic attributes can be found [here](../../../api/build/dynattrs)

### 4. Define dynamic action impl function

We need to declare all arguments defined in step3 and bxl context.

**Note:** The name of the first argument in the impl function must be `bxl_ctx`.

We can read the `file` content by `read_string()` or `read_json()`. Details can
be found [here](../../../api/build/ArtifactValue/)

```python
def process_dynamic_impl(
    bxl_ctx: bxl.Context,          # BXL context
    file: ArtifactValue,           # Dynamic input to read
    output: OutputArtifact,        # Output to write
    my_data: MyData                # Additional args used in the dynamic action
):
    # Read content of dynamic artifact
    content = file.read_string()

    # Process content and run actions
    processed_content = ...
    bxl_actions = bxl_ctx.bxl_actions().actions
    bxl_actions.write(output, processed_content)
    return []
```

### 5. Use it in your bxl main function

```python
def _main(bxl_ctx: bxl.Context):
    # Prepare input/output artifacts
    input_file = ...
    output = bxl_actions.declare_output("output.txt")

    bxl_actions.dynamic_output_new(
        process_dynamic(
            dep_file = input_file,
            output = output.as_output()
        )
    )

    # Don't forgot ensure it to get the output to be materialized
    ctx.output.ensure(output)
```

## Limitations

- `ctx.output` is not available from a dynamic lambda. This means you can’t
  ensure artifacts or print cached outputs within a dynamic lambda.
- Error messages from skipping incompatible targets are only emitted to the
  console, and not cached in the stderr
- `build()` is not available from a dynamic lambda
- `bxl_actions` in a dynamic lambda always inherits the execution platform
  resolution of the root/parent BXL.
  - The expected usage of `bxl_actions` from within a dynamic lambda is to
    instantiate it without any named parameters, but the `exec_deps` and
    `toolchains` of the execution platform resolution are accessible, and return
    the same values as the root/parent BXL
- Profiling is not hooked up to dynamic BXL context

## Examples

Examples can be found at 'tests/core/bxl/test_dynamic_new_data/dynamic.bxl' of
buck2 repo folder.

Run such command at `tests/core/bxl/test_dynamic_new_data/` to run the example
bxl script

```sh
buck2 bxl dynamic.bxl:basic
```
