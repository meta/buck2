---
id: faq
title: FAQs
---

## When is my BXL script cached?

The entire BXL script is represented as a single node on the DICE graph (Buck2’s
internal dependency graph). When the script’s input changes, the entire node is
invalidated and needs to be recomputed. For example, if a BXL function calls
uquery, then uses the result to do a cquery and then a build, if Buck2 detects
that any of the recorded calls to uquery, cquery, and build changes, then the
entire BXL script will be reran. The computations themselves (uquery, cquery,
and build) will still be incrementally evaluated via DICE, so we are not
rerunning _every_ computation entirely within the BXL.

When the BXL script creates artifacts and ensures them, those artifacts are
cached separately in an action outside of the BXL execution. This means that the
artifacts produced by BXL are cached separately from the BXL script itself, much
like the computations within a BXL.

## What’s the difference between `ctx.output.print()` and `print()`?

- `ctx.output.print()` writes items to stdout by buck2 even when the script is
  cached. Items written to the output stream are considered to be the results of
  a BXL script, which will be displayed to stdout by buck2 even when the script
  is cached.
- `print()` is offered by Starlark via the stdlib. This prints anything you want
  but won’t be provided to stdout at the end of a BXL script. These can be used
  to print to stderr. NOTE: `print()` statements don't show up if the script has
  been cached.

## What do I need to know about ensured artifacts

A `bxl.EnsuredArtifact` prints out the relative or absolute path via
`ctx.output.print()`, depending on if called with `abs_path()` or `rel_path()`,
but will print out
`<ensured <build artifact f.txt bound to root//faq.bxl:main>>` via `print()`.

This is intentional because when the ensured artifact is created within BXL, it
has not been materialized yet. It will be materialized after the BXL script
finishes executing, and Buck2 core performs some additional actions after the
BXL script.

This is a safeguard to prevent people from misusing the artifact path and
passing it into an action without the artifact having been materialized or
passing an absolute path into remote execution, which can actually mess up
remote execution and render the action not shareable across users. In addition,
it makes these actions separately cacheable from the BXL execution.

## What is the difference between dynamic outputs and anon targets?

Dynamic outputs are meant for
[dynamic dependencies](../rule_authors/dynamic_dependencies.md). The context
type is a `bxl.Context`. Dynamic outputs are ran asynchronously outside of the
BXL execution.

Anon targets are meant for sharing work between multiple BXLs. The
implementation of a `bxl.anon_rule` receives a `bxl.Context`, and the
implementation of an anon target made from a normal `rule` receives a normal
rule analysis context. Anon targets are resolved inline with your BXL function,
with `ctx.resolve()`.

## Can I mutate types returned by BXL APIs?

The BXL data types, such as nodes, labels and target sets, are immutable. Lists
and dicts returned by methods such as `get_attrs()` are ordinary Starlark values
and can be mutated.

## What is run synchronously vs asynchronously?

Starlark itself is run synchronously. However, certain BXL APIs are evaluated
asynchronously.

If you pass in multiple inputs to builds, queries, or analyses, the execution of
these API calls will be blocking, but the inputs themselves will be evaluated in
parallel within the execution.

Ensuring artifacts and dynamic outputs will happen _after_ the Starlark script
is executed.
