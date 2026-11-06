# Bxl support for performing analysis on targets

## Intro

As Bob and I continue to build out `bxl` we want users to be able to inspect the
providers and actions for a given target label. In order to support this, we
need to be able to provide access to `AnalysisResult` via `starlark`, obtained
via a call to `RuleAnalysisCalculation::get_analysis_result`.

## How to implement it?

Our three principle options are as follows:

1. `BxlContext::analyze(targetlabel: ConfiguredTargetLabelLike)`, where
   `ConfiguredTargetLabelLike` accepts `ConfiguredTargetLabel`,
   `ConfiguredTargetNode`, or sets and lists of these things + acceptable
   strings.

In this scenario, we attach the analysis method onto the bxl context itself, and
require that users pass in the target label-ish thing when they want to
construct an analysis result. It's a little awkward in some ways because the
analysis is more naturally a method on the argument being passed in and the
`BxlContext` is a context that is needed to perform the calculation. On the
other hand, this allows us to construct a type analogous to `TargetExpr` which
can parse from a wide variety of different `ConfiguredTarget` like things
(strings, nodes, labels, sets, ...). It also is a bit nice from an
implementational standpoint since we don't have to pass the context around
everywhere. This isn't a huge pro though, since we can stick it in the global
eval field.

```python
result = bxl.analyze(bxl.cquery.deps("foo"))
```

2. `ConfiguredTargetLabel::analyze()`, `ConfiguredTargetNode::analyze()`, ...
   where we carry around the `BxlContext` in the `eval` global field and
   implement analysis on each type that is target label like.

The pro of this one is that it's quite natural - you can take a
`ConfiguredStarlarkTargetLabel` and then just ... call `analyze()` on it like
you might expect to. The two downsides are that we have to propagate the context
around behind the scenes, and we'll have to provide an implementation of
`analyze` on everything that we'd like to have be able to be `analyzable`.

```python
result = "root//bin:the_binary".analyze()
# but we don't support
"root//bin:the_binary".rdeps()


# instead this looks nice
nodes = ctx.cquery.deps("foo")
for n in nodes:
  # since we can now do
  nodes.label
  nodes.attrs.field

  # similarly access analysis
  nodes.analysis
```

3. `BxlContext::analysis(): AnalysisContext` where `AnalysisContext` exposes
   `AnalysisContext::analyze(targetlabel: ConfiguredTargetLabelLike)`.

There's not really any pros of this approach except that it's similar to the
flow for `cquery` where we return a `cqueryctx` object to call `cquery` methods
through.

```python
result = ctx.analysis().analyze("//lib:file1")
```

We can also restrict the API to require that users go through `cquery` to obtain
a `ConfiguredTargetNode` prior to calling `analysis`, although we don't _have
to_. I say that we don't have to because the `get_analysis_result` method
mentioned above is configured to accept a label anyway.
