---
id: basics
title: BXL Basics
---

This page is a primer on common BXL functionalities and data types. Ramping up
in BXL may be challenging without much prior knowledge of Buck2 building blocks
(ex: targets, configurations, queries), so please take a look at the
[Concepts](../../concepts/concept_map.md) documentation before reading on.

## Common BXL functionalities

### Build

You can build targets within BXL with
[`ctx.build()`](../../../api/bxl/Context/#contextbuild). The result is a
[`bxl.BuildResult`](../../../api/bxl/BuildResult), which has `artifacts()` and
`failures()` functions that provide iterators to the artifacts or failures,
respectively. You can pass in a single target or target pattern to build.

### Analysis

You can run analysis on targets within BXL via
[`ctx.analysis()`](../../../api/bxl/Context/#contextanalysis). Analysis means to
evaluate the underlying rule implementation for the inputted targets, and
produce the providers that the rule defined for the target. A common workflow is
to inspect the resulting providers, and perhaps ensure parts of these providers
or run actions using information from the providers (see [Actions](#actions)
below).

### Query

Buck2 supports a couple different query types: querying the unconfigured graph
(`buck2 uquery`), the configured graph (`buck2 cquery`), or the action graph
(`buck2 aquery`). These queries are all available in BXL as well:

- `ctx.uquery()` returns a [`bxl.UqueryContext`](../../../api/bxl/UqueryContext)
- `ctx.cquery()` returns a [`bxl.CqueryContext`](../../../api/bxl/CqueryContext)
- `ctx.aquery()` returns a [`bxl.AqueryContext`](../../../api/bxl/AqueryContext)

You can read more about the individual queries in the API docs. There are many
queries that are common between uquery, cquery, and aquery, but cquery and
aquery will have extra queries unique to the configured graph or the action
graph. One more thing to call out is the `eval()` query, which is a special
query that takes in the entire query as a string literal. A common use for
`eval()` is to migrate a complex query from Buck2 CLI to BXL by dropping the
entire query string directly into `eval()`.

The query results are target sets (iterable container) of
[`bxl.UnconfiguredTargetNode`s](../../../api/bxl/UnconfiguredTargetNode) for
uquery, [`bxl.ConfiguredTargetNode`s](../../../api/bxl/ConfiguredTargetNode) for
cquery, and [`bxl.ActionQueryNode`s](../../../api/bxl/ActionQueryNode) for
aquery. Each of these node types have accessors on their attributes. A common
workflow is to run some query in BXL, and iterate through the resulting nodes to
inspect their attributes, and use those attributes to inform further
computations in BXL.

#### Uquery

Querying the unconfigured graph means that no configurations (such as platforms
and transitions) have been applied to the target graph yet. This means that it's
very possible that some parts of the target graph is broken due to lack of
configurations. Generally to avoid this problem, cquery may be preferred
instead.

#### Cquery

Querying the configured graph means that configurations have been applied to the
target graph. For cquery, we require that users use a
[target universe](../../how_tos/how_to_use_target_universe) for their query
inputs.

#### Aquery

Aquery is a quite different from uquery and cquery. It is used to query the
action graph, which is constructed after Buck2 runs analysis on the targets and
produces the list of providers and actions needed to build the target.

### Actions

You can create actions directly within the BXL API. The available action APIs
are equivalent to the ones found on the
[`AnalysisActions`](../../../api/build/AnalysisActions) type for normal rules,
with the caveat that
[dynamic actions](../../how_tos/how_to_run_actions_based_on_the_content_of_artifact)
use the [`bxl.Context`](../../../api/bxl/Context) (which provides richer
functionalities).

A common workflow would be to run analysis on a target, and use some interesting
bits found in the analysis result to construct an augmented
[`cmd_args`](../../../api/build/cmd_args) to run, and then ensure the action's
output (see below for ensuring). Also see
[Running actions](../../how_tos/basic_how_tos#running-actions).

### Ensure

Ensuring an artifact means that you want the artifact to be materialized
(meaning, downloaded to your machine) at the end of the BXL execution. There are
two APIs for ensuring: `ctx.output.ensure()` and `ctx.output.ensure_multiple()`
(see [`bxl.OutputStream`](../../../api/bxl/OutputStream)). As the naming
indicates, the former is for ensuring a single artifact, and the latter is for
ensuring multiple artifact-like inputs. Artifact-like inputs include
[`cmd_args`](../../../api/build/cmd_args) (can be found when inspecting
providers), [`bxl.BuildResult`](../../../api/bxl/BuildResult) (produced when
building something in BXL), or [`artifact`](../../../api/build/Artifact) (can be
found when inspecting providers, or creating your own actions).

A common workflow is to ensure an artifact that you created via some custom
actions defined in your script, or ensuring some artifacts found in the
providers after running analysis. Also see
[What do I need to know about ensured artifacts](../../faq#what-do-i-need-to-know-about-ensured-artifacts).
