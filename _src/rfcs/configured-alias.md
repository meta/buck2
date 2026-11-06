# Buck support to implement `configured_alias`

## Intro

Currently, Buck 2 lacks `configured_alias` rule support.

`configured_alias` is a builtin rule in Buck v1, and it cannot be currently
implemented as user defined rule in Buck v2.

This RFC proposes Buck core support for `configured_alias`.

## What is `configured_alias`?

Syntax is this:

```python
configured_alias(
    name = "foo-but-linux-release",
    actual = ":foo",
    platform = "config//platforms:linux-release",
)
```

When this rule is built, it ignores "current" target configuration, and builds
the "actual" target with the configuration specified as "platform" argument.

## How to implement it in buck v2?

### New rule attribute type: `configured_dep`

Currently, we have several dependency attributes:

- `attrs.dep`
- `attrs.exec_dep`
- `attrs.transition_dep`
- `attrs.split_transition_dep`

This RFC proposes adding another attribute:

- `attrs.configured_dep`

`configured_dep` is an attribute which accepts a pair of strings: target and
configuration. During analysis, configured attr deps are resolved to providers
resolved using given configuration.

### `configured_alias_impl` user defined rule

The rule implementation is trivial:

```python

def _configured_alias_impl(ctx):
    return ctx.attrs.actual.providers

configured_alias_impl = rule(
    impl = _configured_alias_impl,
    attrs = {
        "actual": attrs.configured_dep(),
    }
)
```

### Finally, `configured_alias` macro

```python
def configured_alias(name, actual, platform):
    configured_alias_impl(name, actual = (actual, platform))
```

## Alternatives

### No `configured_alias`

Each specific case where `configured_alias` is used, it can be done with
defining custom transition, and using custom transition rule.

But having `configured_alias` is a convenient stopgap to unblock people.

### Use `@configuration` syntax from [another RFC](https://www.internalfb.com/diff/D35136639).

Instead of passing `confiured_target_label(x, y)` pass `x + "@" + y`.

### Accept `configured_target_label` in `dep` attribute

`dep` attribute could support all of:

- regular target label as string
- configured target label (as either `configured_target_label` or `x@y`

I don't know practical applications for this magic, and unless there are uses
for it, better keep API simple and explicit.
