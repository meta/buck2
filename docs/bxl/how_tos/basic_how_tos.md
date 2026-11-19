---
id: basic_how_tos
title: Basic How-Tos
---

import { FbInternalOnly } from 'docusaurus-plugin-internaldocs-fb/internal';

## Writing a BXL

To create a BXL, first, create a script somewhere in the repository ending in
`.bxl`. (Note that you can define a single bxl per file, or multiple BXLs per
file like in Starlark rules).

In it, define a BXL function as follows:

```python
def _your_implementation(ctx):
    # ...
    pass

your_function_name = bxl_main(
    impl = _your_implementation,
    cli_args = {
        # cli args that you want to receive from the command line
        "bool_arg": cli_args.bool(),
        # cli_args will be converted to snakecase. e.g. for this case, passed as --list-type, accessed via ctx.cli_args.list_type
        "list-type": cli_args.list(cli_args.int()),
        "optional": cli_args.option(cli_args.string()),
        "target": cli_args.target_label(),
    },
)
```

This exposes `your_function_name` as a function, with whatever arguments you
defined it, so that on the command line you can invoke:

```sh
buck2 bxl //myscript.bxl:your_function_name -- --bool_arg true --list-type 1 --list-type 2 --target //foo:bar
```

The implementation function takes a single context as parameter (see the
documentation for [`bxl.Context`](../../../api/bxl/Context)). Using it, you'll
be able to access functions that enable you to perform queries, analysis,
builds, and even create your own actions within BXL to build artifacts as part
of a BXL function.

## Running a BXL

To run a BXL function, invoke the buck2 command:

```text
buck2 bxl <bxl function> -- <function args>
```

Where `<bxl function>` is of the form `<cell path to function>:<function name>`,
and `<function args>` are the arguments that the function accepts from the
command line.

The documentation for a BXL function can be seen by running:

```text
 buck2 bxl <bxl function> -- --help
```

Note that this is different from `buck2 bxl --help`, which generates the help
for the buck2 command instead of the function.

## Return information from BXL

The primary method to return information from BXL is to either print them, or
build some artifact (for details, see the
[`bxl.OutputStream`](../../../api/bxl/OutputStream) documentation, available as
part of `ctx.output`). At high level, `ctx.output.print(..)` prints results to
stdout, and `ctx.output.ensure(artifact)` marks artifacts as to be materialized
into buck-out by the end of the BXL function, returning an object that lets you
print the output path via `ctx.output.print(ensured)`.

## Passing in and using CLI args

A BXL function can accept a `cli_args` attribute where args names and types are
specified to use within your script, as shown in the following example:

Example:

```python
def _impl_example(ctx):
    # ...
    pass

example = bxl_main(
    impl = _impl_example,
    cli_args = {
        # cli args that you want to receive from the command line
        "bool_arg": cli_args.bool(),
        "list_type": cli_args.list(cli_args.int()),
        "optional": cli_args.option(cli_args.string()),
        "target": cli_args.target_label(),
    },
)
```

On the command line, you can invoke the arguments as follows:

```sh
buck2 bxl //myscript.bxl:example -- --bool_arg true --list_type 1 --list_type 2 --target //foo:bar
```

For BXL functions, to read the arguments, use them as attributes from the
`cli_args` attribute on the BXL `ctx` object, as follows:

```python
def _impl_example(ctx):
    my_bool_arg = ctx.cli_args.bool_arg
```

## Running actions

You can create actions within BXL via the `actions_factory`. This is called once
globally then used on demand:

```python
def _impl_example(ctx):
    actions = ctx.bxl_actions().actions # call once, reuse wherever needed
    output = actions.write("my_output", "out")
```

You will need to have
[execution platforms](../../rule_authors/configurations.md#execution-platforms)
enabled for your project, or else you will get an error. You can specify the
execution platform resolution by setting named parameters when instantiating
`bxl_actions`:

- `exec_deps` - These are dependencies you wish to access as executables for
  creating the action. This is usually the same set of targets one would pass to
  rule's `attr.exec_dep`. Accepts a list of strings, subtarget labels, target
  labels, or target nodes.
- `toolchains` - The set of toolchains needed for the actions you intend to
  create. Accepts a list of strings, subtarget labels, target labels, or target
  nodes.
- `target_platform` - The intended target platform for your toolchains. Accepts
  a string or target label.
- `exec_compatible_with` - Explicit list of configuration nodes (like platforms
  or constraints) that these actions are compatible with. This is the
  `exec_compatible_with` attribute of a target. Accepts a list of strings,
  target labels, or target nodes.

If you specify `exec_deps` or `toolchains`, you can access the resolved
`dependency` objects on the `bxl_actions` object. The `bxl_actions` object will
have `exec_deps` and `toolchains` attributes, which are `dict`s where the keys
are the unconfigured subtarget labels, and the values are the
configured/resolved `dependency` objects.

Note that the keys of `exec_deps` and `toolchains` must be unconfigured
subtarget labels (`StarlarkProvidersLabel`), and not unconfigured target labels.
You can use `ctx.unconfigured_sub_targets(...)` or `with_sub_target()` on
`target_label` to create the label.

```python
def _impl_example(ctx):
    my_exec_dep = ctx.unconfigured_sub_targets("foo//bar:baz") # has some provider that you would use in the action
    bxl_actions = ctx.bxl_actions(exec_deps = [my_exec_dep]) # call once, reuse wherever needed
    output = bxl_actions.actions.run(
        [
            "python3",
            bxl_actions.exec_deps[my_exec_dep][RunInfo], # access resolved exec_deps on the `bxl_actions`
            out.as_output(),
        ],
        category = "command",
        local_only = True,
    )
    ctx.output.ensure(output)
```

## Getting providers from an analysis

After calling `analysis()`, you can get the providers collection from
`providers()`:

```python
def _impl_example(ctx):
    my_providers = ctx.analysis(my_target).providers()
```

## Get a specific provider from an analysis

After calling `analysis()`, you can also get the providers collection from
`providers()` then grab whatever specific provider you need:

```python
def _impl_example(ctx):
    default_info = ctx.analysis(my_target).providers()[DefaultInfo]
    ctx.output.print(default_info)
```

## Get a specific subtarget from an analysis

Once you have a provider, you can get its subtargets by using the `sub_targets`
attribute on the struct to get a dict of provider labels to provider
collections:

```python
def _impl_example(ctx):
    subtarget = ctx.analysis(my_target).providers()[DefaultInfo].sub_targets["my_subtarget"]
    ctx.output.print(subtarget)
```

## Building a target/subtarget without blocking

`ctx.build` is synchronous and should only be used when the result of the build
is needed inline during the bxl execution. To execute builds without blocking
the script, retrieve the outputs from `DefaultInfo` of the target's providers
and use the `ctx.output.ensure/ensure_multiple` for the outputs. Or you can use
the util function
[`ensure_default_info`](../../../api/bxl_utils/ensure/#ensure_default_info)

## Accessing Unconfigured/Configured Target Node Attributes

BXL provides a unified API for accessing attributes on both unconfigured and
configured target nodes.

- [`node.get_attr(key)`](../../../api/bxl/ConfiguredTargetNode/#configuredtargetnodeget_attrs):
  Get one attribute
- [`node.get_attrs`](../../../api/bxl/ConfiguredTargetNode/#configuredtargetnodeget_attrs):
  Get all attributes
- [`node.has_attrs(key)`](../../../api/bxl/ConfiguredTargetNode/#configuredtargetnodeget_attrs):
  Check if one attribute exists

For special attributes like `rule_kind`, we get them directly from node:

```python
node.rule_kind
```

### Deprecated apis

The following attribute access api are not recommended and will be deprecated

For `ConfiguredTargetNode`:

- [`.attrs_eager`](../../../api/bxl/ConfiguredTargetNode/#configuredtargetnodeattrs_eager)
- [`.attrs_lazy`](../../../api/bxl/ConfiguredTargetNode/#configuredtargetnodeattrs_lazy)
- [`.resolved_attrs_eager`](../../../api/bxl/ConfiguredTargetNode/#configuredtargetnoderesolved_attrs_eager),
- [`.resolved_attrs_lazy`](../../../api/bxl/ConfiguredTargetNode/#configuredtargetnoderesolved_attrs_lazy)

For `UnconfiguredTargetNode`:

- [`.attrs`](../../../api/bxl/UnconfiguredTargetNode/#unconfiguredtargetnodeattrs)

### Example

```python
def _impl_example(ctx):
    my_configured_node = ctx.configured_targets(":foo")

    # get an attribute named "foo", if not exist return None
    foo_attr = my_configured_node.get_attr("foo")

    # get all attributes, it returns a dict mapping from attribute name to attribute
    all_attrs = my_configured_node.get_attrs()

    # check if "foo" attribute exists on node
    foo_exist = my_configured_node.has_attr("foo")

    # access special attribute `rule_type`
    rule_type = my_configured_node.rule_type

    # same for UnconfiguredTargetNode
```

## Inspecting a struct

You can use `dir(my_struct)` to inspect a struct. You can also use
`getattr(my_struct, "my_attr")` to grab individual attributes, which is
equivalent to `my_struct.my_attr`.

These are available as part of the
[Starlark language spec](https://github.com/bazelbuild/starlark/blob/master/spec.md#dir).

## Set addition/subtraction on a `target_set`

There are a few BXL actions that return a `target_set` (such as a cquery
`eval()`). The `target_set` supports set subtraction and addition (you can use
`-` and `+` directly in Starlark).

## Initializing configured/unconfigured `target_set`

You can use following apis to initialize `target_set`

```python
def bxl.utarget_set(nodes: None | list[bxl.UnconfiguredTargetNode]) -> bxl.UnconfiguredTargetSet
```

```python
def bxl.ctarget_set(nodes: None | list[bxl.ConfiguredTargetNode]) -> bxl.ConfiguredTargetSet
```

## Profiling, Testing, and Debugging a BXL script

You can use `buck2 bxl profiler`, with various measurements, to determine where
the script is least efficient.

To time individual pieces of the script, you can use BXL’s timestamp methods:

```python
def _impl_example(_ctx):
    start = now() # call once and reuse wherever is necessary
    # do something time intensive here
    end1 = start.elapsed_millis()
    # do something else time intensive here
    end2 = start.elapsed_millis()
```

- **Debug** - the common way to debug a BXL script is with print statements
  (`print()`, `pprint()` and `ctx.output.print()`).

<FbInternalOnly>

- **Debugger** - to use the debugger you can follow these instructions
  [here](https://fb.workplace.com/groups/buck2eng/permalink/3562907607330619/).

      1. `fdb --starlark-kill-buck attach buck`
      2. place a breakpoint to the bxl file
      3. run the buck2 bxl command

</FbInternalOnly>

- **Test** - BXL does not have a robust testing framework for mocking. The main
  method to test a BXL script is to actually invoke it with required inputs then
  verify the outputs.

## Getting the path of an artifact as a string

The starlark `artifact` type encapsulates source artifacts, declared artifacts,
and build artifacts. It can be dangerous to access paths and use them in further
BXL computations. For example, if you are trying to use absolute paths for
something and end up passing it into a remotely executed action, the absolute
path may not exist on the remote machine. Or, if you are working with paths and
expecting the artifact to already have been materialized in further BXL
computations, that would also result in errors.

However, if you are not making any assumptions about the existence of these
artifacts, you can use
[`get_path_without_materialization()`](../../../api/bxl#get_path_without_materialization),
which accepts source, declared, or build artifacts. It does _not_ accept ensured
artifacts (also see
[What do I need to know about ensured artifacts](../../faq#what-do-i-need-to-know-about-ensured-artifacts)).

For getting paths of `cmd_args()` inputs, you can use
[`get_paths_without_materialization()`](../../../api/bxl#get_paths_without_materialization),
but note this is risky because the inputs could contain tsets, which, when
expanded, could be very large. Use these methods at your own risk.
