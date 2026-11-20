---
id: how_to_use_target_universe
title: How to Use Target Universe in BXL
---

## BXL cquery and target universe

BXL cannot infer the
[target universe](../../concepts/glossary.md#target-universe) like in the CLI
(in most cases). BXL splits up cquery functions per function (ex:
`ctx.cquery().kind(...)`), with the exception of `ctx.cquery.eval(...)`, which
accepts literals exactly like in the CLI. For the `eval` query, target universe
is inferred exactly like the CLI.

For all other cases, take the following query as an example:

`buck2 cquery "rdeps(deps(//example:foo), deps(//example:bar))"`

The target universe here should be constructed from the all the target literals
and their transitive deps, which is to say `deps(//example:foo, //example:bar)`.
When you run the query, the evaluation of `deps(//example:foo)` and
`deps(//example:bar)` nested in the `rdeps` query will happen inside the
universe resulting from `deps(//example:foo, //example:bar)`. Translating it to
BXL's individual cquery functions, and let’s say we also try to use the target
literals to construct the universe as the CLI target inference does:

```python
from_node = ctx.cquery().deps("//example:foo") # universe would be //example:foo

to_node = ctx.cquery().deps("//example:bar") # universe would be //example:bar

rdeps = ctx.cquery().rdeps(from_node, to_node) # what is the universe here?
```

Here, the `from_node` query is actually evaluated in the wrong target universe
because we have broken up the query steps in BXL. Instead of
`deps(//example:foo)` being evaluated in `deps(//example:foo, //example:bar)`,
it’s evaluated with only `deps(//example:foo)`. It’s impossible to know that
there’s going to be an rdeps query later on that expects a different target
universe.

### Specifying target universe in BXL cquery

BXL cquery functions should only accept configured targets as inputs, with the
exception of `eval` and `testsof_with_default_platform`.

BXL has a `ctx.target_universe()` function to construct a `target_universe`
object, which has a `lookup()` function to lookup the configured targets within
the target universe and return the target set. ​​The lookup functionality is
useful because sometimes a single target can appear multiple times within a
target universe. For example, if you specify a cxx toolchain using its
unconfigured target label, it will always match against all cxx toolchains in
the target universe (so at least once for target deps and once for exec deps),
since cxx toolchains may have multiple configurations. Example:

```python
def _impl():
    target_universe = ctx.target_universe(["//example:foo", "//example:bar"])
    to_node = target_universe.lookup("//example:foo")
    from_node = target_universe.lookup("//example:bar")
    rdeps = ctx.cquery().rdeps(to_node, from_node)
```

However, sometimes you might want a specific configuration instead of using all
configurations found within a target universe, in which case you could use
`ctx.configured_targets(...)` to specify the configuration. Or, sometimes you
may want to use the specific configured target nodes resulting from other BXL
calls. In these cases, you can pass the configured targets directly into cquery
functions, instead of going through target universe lookup.

### What does the target universe tend to be in practice?

For `owner` query, the universe would be constructed with the unconfigured
target nodes returned from `ctx.uquery().owner(...)`. Example:

```python
def _impl():
    unconfigured_owners = ctx.uquery().owner("foobar")
    target_universe = ctx.target_universe(unconfigured_owners).target_set()
    owners = ctx.cquery().owner("foobar", target_universe)
```

For everything else, the universe would usually be constructed using all target
literals found in your query. Example:

```python
def _impl():
    target_universe = ctx.target_universe("//example:foo")
    inputs = target_universe.target_set()
    deps = ctx.cquery().deps(inputs)
```

While the above guideline should work for `rdeps` as well, for `rdeps` the
universe would usually be narrowed down to the "to"/"destination" target set
argument. (This is a subset of the target universe suggested for non-`owner`
query cases). Updating the example from above:

```python
def _impl():
    target_universe = ctx.target_universe("//example:foo") # narrowed down to the "to" literals in rdeps
    universe_node = target_universe.target_set()
    from_node = target_universe.lookup("//example:bar")
    rdeps = ctx.cquery().rdeps(universe_node, from_node)
```

### `keep-going`

The configured graph can be broken for various reasons: incompatible targets
(BXL skips these automatically), visibility issues, nonexistent targets, etc.
For issues that are not incompatible targets, the `target_universe` can be
constructed with the `keep_going` flag set to `True` to skip any other errors,
and your cquery will not error out.

```python
ctx.target_universe("//foo/...", keep_going = True)
ctx.target_universe(my_target_set, keep_going = True)
```

## BXL build and target universe

Note that BXL builds currently do not support target universe, but we intend to
add this.
