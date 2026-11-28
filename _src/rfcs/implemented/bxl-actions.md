# Bxl Actions and Build API

Bxl allows integrators to write Starlark snippets that introspect the buck2
graph, and perform various operations on them within Starlark to accomplish
complex operations, as previously proposed in [bxl RFC](../bxl))

This document is intended at discussing the aspects of build and actions
declaration of a bxl function in more details, and proposed changes to deferred
framework to support bxl actions.

## Actions API

The actions API should be the same as rules' actions API. That is, it has the
same `ctx.actions` that allows registering of artifacts, creating actions,
dynamic actions via the same api.

## Creating and Building the Actions

Bxl allows users to build targets and actions. However, when creating actions,
they are not bound/buildable until the artifact/action factories are finalized.
As such, we will introduce the limitation that bxl cannot build artifacts that
they themselves declared within the bxl. Instead, they will return a set of
artifacts to expose to users, which buck2 will automatically build after
finalizing the action factory. For dynamic-ness, bxl users will use the standard
dynamic output api. There is an issue that during the dynamic output api's
lambda, bxl functions will not be able to access the regular bxl functions for
queries, etc. However, this is likely not important as most use cases should
reasonably query bxl data before the dynamic outputs, and have limited power in
dynamic-ness. We can also always replace the ctx of the dynamic to be the bxl
context in the future, as we see fit.

Sample:

```python
def my_bxl(ctx):
    actions_factory = ctx.bxl_actions.factory()

    artifact = actions_factory.write("file.txt", "content")

    # note that existing artifacts that were declared by other rules can be built
    ctx.actions.build(ctx.analysis(ctx.target("foo")).providers[DefaultInfo].default_output))

    return [artifact] # exposes the declared artifact to users
```

## Internal Representation (Deferred Framework)

The existing actions framework attaches all actions to a deferred, which is
based off a `ConfiguredLabel`, which also corresponds to the output path prefix.
bxl actions should also have a unique output path prefix, and follow the same
system of having a base deferred key to reuse the action implementation.

We should extend the `BaseKey` of a `DeferredKey` to support beyond a
`ConfiguredLabel`, so that we can use a `BxlFunctionLabel` in its place. This
would allow `owner` of these actions to point to the correct creator. The output
path would be determined by using the `BxlFunctionLabel` as prefix similar to a
label. While this means that not all outputs are associated with an actual rule,
this is arguably more correct as bxl that creates outputs that doesn't fit the
target graph structure (i.e android project generation follows directory
structure rather than the packages defined by targets) to not have to conform
the attaching their actions to existing rules. bxl functions can examine
multiple rules and create a single action, attached only to their function
label.

The ActionRegistry will be attached to the evaluation result of `bxl`. Since we
do not allow bxl to explicitly request build of the actions itself declares, we
can wait until the end of the bxl function to finalize the actions. Then, the
action lookup can simply refer to the result of the `bxl`.

With the above changes, the rest of the actions framework does not need changed
to support the proposed API. DICE caching will work as today.
