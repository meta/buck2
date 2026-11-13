# Shortcomings with Existing Features

Below are the four potential approaches for handling the [nccl version + sanitizer problem](main.md#problem) and where each one falls short.

## Case 1: always require `-m cfg//nccl:version[latest]` to be used with `-m <sanitizer>`

This is the first solution most people jump to, since sanitizer is usually enabled as a CLI modifier and it's easy to stick both modifiers into a modefile for each sanitizer. It fails for two reasons.

1. Requiring multiple CLI modifiers in combination breaks the fundamental UX promise of modifiers, which is that a small set of unified keywords is enough to enable diverse/complex behaviors. Our goal is for simple, unified CLI flags like `-m asan` to work standalone.
2. A dep binary may require a different nccl version to build than its parent binary, so nccl version can change in transition functions on binaries. Passing `-m <sanitizer>` on the CLI doesn't help in the transition case, because the transition will override the sanitizer value set from the CLI.

## Case 2: use conditional modifiers

Conditional modifiers are in theory the modifier feature designed to solve this problem. The feature itself is straightforward. You define a conditional modifier as follows:

```python
# cfg//nccl/BUCK file

conditional_modifier(
  name = "latest_when_sanitized",
  modifier = {
    "cfg//:sanitizer[{}]".format(k): "cfg//nccl:version[latest]"
    for k in ["asan", "tsan", "ubsan"]
  },
)
```

And add it to the root PACKAGE file of the repo.

```python
set_cfg_modifiers([
  "cfg//nccl:latest_when_sanitized",
])
```

In theory, this promotes the nccl version to latest when a sanitizer is enabled.

In practice, it doesn't work unless every PACKAGE file knows to set conditional modifiers for nccl version. Setting the modifier unconditionally immediately overrides the conditional one.

```python
# my_project/PACKAGE

set_cfg_modifiers([
  "cfg//nccl:stable_when_no_sanitizer",  # does not override other conditional modifier
])

# my_project2/PACKAGE

set_cfg_modifiers([
  "cfg//nccl:stable",  # overrides other conditional modifier
])
```

Even if we could solve the above (e.g. by automatically treating `cfg//nccl:stable` as `cfg//nccl:stable_when_no_sanitizer`), it doesn't solve the transition problem. Modifiers are only resolved at modifier resolution time, not at transition time. So a dep binary could transition to a different nccl version when sanitizer is enabled, which breaks the premise.

## Case 3: use a transition function

You could imagine a transition function that always updates the nccl version to latest when sanitizer is enabled.

```python
def _some_transition_impl(ctx: AnalysisContext) -> list[Provider]:
  sanitizer_setting = ctx.attrs.sanitizer_setting[ConstraintSettingInfo]
  none_sanitizer = ctx.attrs.none_sanitizer[ConstraintValueInfo]
  latest_nccl = ctx.attrs.latest_nccl[ConstraintValueInfo]

  def _tr(platform: PlatformInfo) -> PlatformInfo:
    # other logic ...

    current = platform.configuration.constraints.get(sanitizer_setting.label)
    if current == None or current.label == none_sanitizer.label:
        return platform
    new_constraints = dict(platform.configuration.constraints)
    new_constraints[latest_nccl.setting.label] = latest_nccl

    # other logic ...
    return PlatformInfo(
        label = platform.label,
        configuration = ConfigurationInfo(
            constraints = new_constraints,
            values = platform.configuration.values,
        ),
    )

  return [DefaultInfo(), TransitionInfo(impl = _tr)]

some_transition = rule(
  impl = _some_transition_impl,
  attrs = {
    "sanitizer_setting": attrs.dep(default = "cfg//:sanitizer"),
    "none_sanitizer": attrs.dep(default = "cfg//:sanitizer[none]"),
    "latest_nccl": attrs.dep(default = "cfg//nccl:version[latest]"),
    # other attrs...
  },
  is_configuration_rule = True,
)
```

1. There's no way to enforce that this transition is uniformly applied, since there's no way to globally apply a transition function to every binary. If someone leaves out this transition or writes an nccl transition for their project that doesn't include this logic, they don't get the desired behavior.
2. Transitions can't apply to arbitrary libraries, so you can get the wrong version when building a `cxx_library` directly.
3. Transitions are custom Starlark functions and become hard to maintain once the logic gets complex.

There are also simpler ways to define custom transitions without introducing custom Starlark rules. For example, you can use a generic constraint overrides transition with a `constraint_overrides` attr that takes an arbitrary list of constraints to transition on, so you just need to define constraint overrides as follows.

```python
constraint_overrides += select({
  "cfg//:sanitizer[asan]": ["cfg//nccl:version[latest]"],
  "cfg//:sanitizer[tsan]": ["cfg//nccl:version[latest]"],
  "cfg//:sanitizer[ubsan]": ["cfg//nccl:version[latest]"],
  "cfg//:sanitizer[none]": [],
})
```

Note the above example may not work today, because transitions disallow the constraints in select keys from also being transitioned on. This isn't a fundamental restriction, so let's ignore it for now.

Unfortunately, this approach hits the same fundamental problem as conditional modifiers (case #2). Any project that needs to set its own constraint overrides for nccl version can accidentally override this unconditionally by setting `"cfg//nccl:version[stable]"` in `constraint_overrides`. It also still doesn't work with library targets.

## Case 4: use nested selects

None of the above solutions can fully solve the nccl/sanitizer problem. There is one existing solution that does. Suppose an alias is used so that users always depend on the correct nccl library version.

```python
# third-party/nccl/BUCK

alias(
  name = "nccl",
  actual = select({
    "cfg//nccl:version[{}]".format(v): "//third-party/nccl/{}:nccl".format(v)
    for v in ["latest", "beta", "stable"]
  }),
)
```

The alias can be updated to select on sanitizer first and use latest when sanitizer is enabled.

```python
# third-party/nccl/BUCK

alias(
  name = "nccl",
  actual = select({
    "cfg//:sanitizer[{}]".format(s): "//third-party/nccl/latest:nccl"
    for s in ["asan", "tsan", "ubsan"]
  } | {
    "cfg//:sanitizer[none]": select({
      "cfg//nccl:version[{}]".format(v): "//third-party/nccl/{}:nccl".format(v)
      for v in ["latest", "beta", "stable"]
    }),
  }),
)
```

This is a fairly small change, just adding an extra layer of nesting in the select, but it works for all the edge cases above. The nccl version picked is always correct, no matter how nccl version changes via transitions or even deprecated methods like `configured_alias`.

However, this leaves a major footgun: **selecting on nccl version directly is now wrong, because it may return the incorrect version**. Any user can introduce a select on nccl version directly in their project to do something version-specific, and it will silently produce the wrong result. We have no good way to enforce that users select on sanitizer first.

In practice, we trip over this footgun *all the time*. For example, default link style is a constraint that depends on many other constraints like OS and sanitizer, so its selects are deeply nested. Many users have introduced selects on default link style in the repo that forget to nest the OS and sanitizer checks, so those selects almost certainly behave differently than intended.

A second problem with nested selects is that constraints depending on many other constraints require a huge number of nesting levels. In practice this creates extremely bloated selects in macros. They're hard to read, and they cost a lot of memory for Buck to store.
