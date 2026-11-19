---
id: configuration_transitions
title: Configuration Transitions
---

This page mostly focuses on how configuration transitions are
implemented. A good understanding of the high-level
[concepts](../concepts/transitions.md) is therefore required.

## Defining transitions

The meat of any transition definition is the transition implementation, a
function which accepts the pre-transition `PlatformInfo` as an argument, and
returns the modified `PlatformInfo` that should be transitioned to. Here's an
example of what such a function might look like:

```python
def _transition_impl_with_refs(platform: PlatformInfo) -> PlatformInfo:
    # Not bound in this function, see below for where these come from
    os = os[ConstraintSettingInfo]
    watchos = watchos[ConstraintValueInfo]
    # Remove operating system constraint from input platform.
    constraints = {
        s: v
        for (s, v) in platform.configuration.constraints.items()
        if s != os.label
    }
    # Add watchos constraint value.
    constraints[watchos.setting.label] = watchos
    # Construct configuration structure.
    new_cfg = ConfigurationInfo(
        # Updated constraints.
        constraints = constraints,
        # Keep original config values.
        values = platform.configuration.values,
    )
    # And return new configuration,
    # or a dict of marker to configuration in case of split transition.
    return PlatformInfo(
        # ... supplying configuration label.
        label = "<transitioned-to-watch>",
        configuration = new_cfg,
    )
```

Much like constraints and platforms, transitions make their way into the graph
by means of a target that returns a built-in provider - specifically, the
`TransitionInfo` provider. The `TransitionInfo` provider accepts only one
parameter, `impl` a callable with signature `PlatformInfo -> PlatformInfo`. With
that in mind, we can a transition rule:

```python
def _transition_to_watchos_impl(_ctx: AnalysisContext) -> list[Provider]:

    # From above
    def _transition_impl_with_refs(platform: PlatformInfo) -> PlatformInfo:
        # Not bound in this function, see below for where these come from
        os = os[ConstraintSettingInfo]
        watchos = watchos[ConstraintValueInfo]
        ...

    return [
        DefaultInfo(),
        TransitionInfo(
            impl = _transition_impl_with_refs,
        ),
    ]

transition_to_watchos = rule(
    impl = _transition_to_watchos_impl,
    attrs = {},
    # Rules that define transitions must be configuration rules
    is_configuration_rule = True,
)
```

Most transition functions will require access to the analysis outputs of other
configuration rules, typically to extract a `ConstraintSettingInfo`,
`ConstraintValueInfo`, or `PlatformInfo` from them. The example above does as
well - it needs the `os` constraint setting and `watchos` constraint value.

Analysis results from other configuration rules are made available by depending
on those rules as dependencies like in any other analysis. We can use that to
finish the example above:

```python
def _transition_to_watchos_impl(ctx: AnalysisContext) -> list[Provider]:
    os = ctx.attrs.os
    watchos = ctx.attrs.watchos

    # From above
    def _transition_impl_with_refs(platform: PlatformInfo) -> PlatformInfo:
        # These values are captured into the `def` from above
        os = os[ConstraintSettingInfo]
        watchos = watchos[ConstraintValueInfo]
        ...

    return [
        DefaultInfo(),
        TransitionInfo(
            impl = _transition_impl_with_refs,
        ),
    ]

transition_to_watchos = rule(
    impl = _transition_to_watchos_impl,
    attrs = {
        "os": attrs.dep(default = "//constraints:os"),
        "watchos": attrs.dep(default = "//constraints:watchos"),
    },
    is_configuration_rule = True,
)
```

### Idempotence

A transition function applied twice must produce the configuration identical to
the configuration produced after applying transition once. Violating this
requirement is an error.

```python
assert tr(tr(platform=platform)) == tr(platform=platform)
```

## Incoming edge transitions

With a suitable transition target defined, you can set an incoming edge
transition on a target by passing the transition target to the built-in
`incoming_transition` attribute, like this:

```python
# BUCK
transition_to_watchos(
    name = "transition_to_watchos",
)

my_binary(
    name = "watchos_binary",
    deps = ...
    incoming_transition = ":transition_to_watchos",
)
```

`incoming_transition` attributes are not available on all rules - instead, rules
must declare that they support them by setting
`supports_incoming_transition = True` as a parameter to the `rule` call

## Outgoing edge transitions

Outgoing edge transitions are declared via use of
`attrs.transition_dep(cfg = ":transition_target")`. Such attributes act much
like an `attrs.dep()`, except that the transition is applied.

## Access rule attributes in transition function implementation

It might be useful for the transition function to be able to query rule
attributes (for example, to perform transition to different configurations
depending on `java_version` attribute).

Both incoming (per rule) and outgoing (per dependency) transitions can access
rule attributes. For outgoing transitions, transition rule implementation
accesses the attributes of the target that has dependencies with transitions,
not attributes of dependency targets.

```python
def _tr(platform, attrs):
    # NB: There are some restrictions on what attrs can be made accessible:
    # - Only primitive values for now (providers are not resolved)
    # - Only unconfigured attributes for now
    attrs.my_list_attribute # == [12345, 67890]


def _transition_target_impl(ctx):
    return [
        DefaultInfo(),
        TransitionInfo(
            impl = _tr,
            attrs = {
                "my_list_attribute": attr.list(...),
            },
        ),
    ]
_transition_target = rule(
    impl = _transition_target_impl,
    is_configuration_rule = True,
)

_transition_target(
    name = "my_transition_target",
)

my_rule = rule(..., supports_incoming_transition)

my_rule(
    ...,
    my_list_attribute = [12345, 67890],
    incoming_transition = ":my_transition_target",
)
```

## Deprecated transition declarations with `transition` objects

There is an old, soft-deprecated mechanism to declare transitions that used
objects returned by the `transition` object. These work substantially similarly
with the objects replacing transition targets, but there are some key
differences (uquery correctness bugs, first class refs, etc.).

The new API is strictly more powerful - it should be used instead in new code.

## Split transitions

Along with the old transition API, there is first class support for a notion of
"split transitions." It is currently unclear whether split transitions will
remain supported as a first-class concept going forward. It is probably wise to
use something along the lines of the following instead:

```python
attrs.tuple(
    attrs.transition_dep(cfg = tr_1),
    attrs.transition_dep(cfg = tr_2),
)
```

For completeness, below are the old docs for split transitions:

For split transition, each dependency is resolved into a dict of marker to
providers.

For example:

```python
android_binary = rule(
    ...
    attrs = {
        "deps": attrs.list(attrs.split_transition_dep(cfg = cpu_split_transition), default = []),
    },
)
```

When the above is invoked as follows:

```python
android_binary(
    deps = ["//foo:bar", "//qux:quux"],
)
```

Then the rule implementation gets something like the following in the `deps`
attribute:

```python
{
    [
        {
            # Key in this dict is the marker returned from split transition impl function.
            "arm64": "providers for //foo:bar configured for arm64",
            "armv7": "providers for //foo:bar configured for armv7",
        },
        {
            "arm64": "providers for //qux:quux configured for arm64",
            "armv7": "providers for //qux:quux configured for armv7",
        },
    ]
}
```

:::note

It is an error to pass a split transition object to `attrs.transition_dep` and a
non-split transition to `attrs.split_transition_dep`.

:::
