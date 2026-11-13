<!-- Source: https://docs.google.com/document/d/1GTgFP0vNo67YLE52ypx9qMgJZtDW7Sl3jqx7KPanekw/ -->

# [RFC] Conditional Constraint

## Problem

Sometimes the value of one constraint should depend on the values of others. This shows up all over the repo: cpp optimization level depends on build mode, default link style depends on OS and sanitizer, MSVC can't be used outside of windows, and so on.

Let's consider the NCCL (NVIDIA Collective Communication Library) version constraint defined below as an example. Our repo supports multiple versions of NCCL, but only the latest version has good sanitizer support, so any sanitizer build should always use the latest NCCL version regardless of what NCCL version was originally set.

```python
# cfg//nccl/BUCK file

constraint(
  name = "version",
  values = ["stable", "beta", "latest"],
  default = "stable",
)

# cfg//BUCK file

constraint(
  name = "sanitizer",
  values = [
    "asan",
    "tsan",
    "ubsan",
    "none",
  ],
  default = "none",
)
```

Existing solutions fall short in several ways (documented in [this tab](problems.md)). This RFC proposes an API that handles this general class of cross-constraint dependency.

## Basic API

This RFC introduces conditional attrs to the [unified constraint rule](https://docs.google.com/document/d/1AydiiQWBhB_VTl07jPZmyApPkjgxbs4-OQU_nC2Pt9M/edit?tab=t.n2ag5qjff01f#heading=h.f1h19vugfrcm). These attrs let users define how a constraint's value should resolve based on the values of other constraints via a `select`.

There are three such attrs:

1. `default`, which lets you define the default value of a constraint based on the value of other constraints.
2. `invariant`, which lets you define a value that the constraint is required to resolve to if it is not set to `None`.
3. `compatibility`, which lets you describe compatibility based on this and other constraints.

### Invariant

To solve the example NCCL problem, we can define an `invariant` field on the NCCL version constraint as follows.

```python
# cfg//nccl/BUCK file

constraint(
  name = "version",
  values = ["stable", "beta", "latest"],
  default = "stable",
  invariant = select({
    "cfg//:sanitizer[asan]": "latest",
    "cfg//:sanitizer[tsan]": "latest",
    "cfg//:sanitizer[ubsan]": "latest",
    "cfg//:sanitizer[none]": None,
  }),
)
```

The select in `invariant` gets evaluated against the current configuration when the version constraint is used in a select. In this case, NCCL version always matches `latest` when a sanitizer is enabled. Otherwise, it matches the existing value set for NCCL version constraint in the configuration. `None` specifies that there is no invariant to follow.

Because `invariant` is evaluated at select time, **no matter how the configuration changes** (ex. modifiers, transitions, default target platform, configured aliases), **latest nccl version on sanitizer builds is always enforced**. This allows constraint authors to define **rules about constraints that cannot be broken** (ex. cannot use MSVC outside of windows).

Additionally, users of nccl don't need to know this special logic exists. They can directly add nccl modifiers or select on the nccl version, and sanitizer variation is already handled for them, thanks to the power of abstraction.

A constraint's invariant may depend on resolving other constraints' invariants. Cycles in dependency are not allowed and will produce an error.

The above example can be simplified with the use of `DEFAULT` in `select`.

```python
# cfg//nccl/BUCK file

constraint(
  name = "version",
  values = ["stable", "beta", "latest"],
  default = "stable",
  invariant = select({
    "DEFAULT": "latest",
    "cfg//:sanitizer[none]": None,
  }),
)
```

### Default

Suppose we want to make latest on sanitizer builds only the default behavior, as opposed to required behavior. This is supported by specifying the same select on the `default` field instead of the `invariant` field.

```python
# cfg//nccl/BUCK file

constraint(
  name = "version",
  values = ["stable", "beta", "latest"],
  default = select({
    "DEFAULT": "latest",
    "cfg//:sanitizer[none]": "stable",
  }),
)
```

Now, NCCL version will resolve to `stable` without sanitizer and `latest` with sanitizer by default, but any project can override these default settings with arbitrary values for both constraints.

Note that we intend for a configuration that uses the default value to be identical to one where that same value is explicitly set. However, for correctness reasons, when the default is a select, the select expression itself is retained rather than its resolved value, so the two configurations won't be identical. For example, in this case, it means that a configuration with NCCL version unset that resolves to stable will never match a configuration with NCCL version explicitly set to stable.

### Compatibility

Suppose NCCL only works on linux. The cleanest way to encode that is via the `compatibility` attr.

```python
# cfg//nccl/BUCK file

constraint(
  name = "version",
  values = ["stable", "beta", "latest"],
  default = "stable",
  compatibility = select({
    "cfg//:os[linux]": True,
    "DEFAULT": False,
  }),
)
```

This lets users concentrate incompatibilities between constraints at a single location, rather than spreading them across many targets in the repo by defining the `target_compatible_with` attr.

The previously mentioned policy of requiring latest on sanitizer can also be encoded in compatibility.

```python
# cfg//nccl/BUCK file

constraint(
  name = "version",
  values = ["stable", "beta", "latest"],
  default = "stable",
  compatibility = select({
    "cfg//:os[linux]": select({
      "cfg//:sanitizer[none]": select({
        "latest": True,
        "stable": False,
      })
    }),
    "DEFAULT": False,
  }),
)
```

`True` represents compatible and `False` represents incompatible. By default, compatibility is set to `True`.

In the above case, compatibility is allowed to reference the value of its own constraint (`latest` and `stable`). This is something that only compatibility is allowed to do; other fields like `default` and `invariant` cannot.

It's likely that we will also allow the use of `select_incompatible` in compatibility so that a user-defined error message can be used in case of incompatibility.

Compatibility can often be verbose. We will likely solve this by adding new select ergonomics improvements (ex. supporting native ANDs).

## Advanced API

### Sub-constraints

Suppose we want to add multiple "policies" for how sanitizer enablement affects the NCCL version.

1. *Force latest*. Always pick the latest version if sanitizer is enabled.
2. *Force beta*. Always pick the beta version if sanitizer is enabled.
3. *Ignore*. Use the set NCCL version regardless of sanitizer enablement.

One way to encode this policy is to add an additional constraint that is used in the invariant of the NCCL version.

```python
# cfg//nccl/BUCK file

constraint(
  name = "sanitizer_policy",
  values = ["force_latest", "force_beta", "ignore"],
  default = "force_latest",
)

constraint(
  name = "version",
  values = ["stable", "beta", "latest"],
  default = "stable",
  invariant = select({
    "cfg//:sanitizer[none]": None,
    "DEFAULT": select({
      ":sanitizer_policy[force_latest]": "latest",
      ":sanitizer_policy[force_beta]": "beta",
      ":sanitizer_policy[ignore]": None,
    }),
  }),
)
```

Unfortunately, the above definition leaks the definition of sanitizer policy to downstream selects, such that anyone can select on sanitizer policy directly instead of reading the NCCL version. In an ideal world, we'd have a way to limit the visibility of sanitizer policy.

This is where a sub-constraint is useful. A sub-constraint adds an extra input dimension used to resolve a constraint. It is a convenience feature to quickly declare that extra dimension, and it also naturally limits the visibility of the sub-constraint to the parent constraint.

```python
# cfg//nccl/BUCK file

constraint(
  name = "version",
  values = ["stable", "beta", "latest"],
  sub_constraints = [
    sub_constraint(
      name = "sanitizer_policy",
      values = ["force_latest", "force_beta", "ignore"],
      default = "force_latest",
    ),
  ],
  default = "stable",
  invariant = select({
    "cfg//:sanitizer[none]": None,
    "DEFAULT": select({
      "sanitizer_policy[force_latest]": "latest",
      "sanitizer_policy[force_beta]": "beta",
      "sanitizer_policy[ignore]": None,
    }),
  }),
)
```

Sub-constraints have tighter semantics than regular constraints:

- A sub-constraint has only `values` and unconditional `default`. It does not support `compatibility`, `invariant`, or a `default` with `select`.
- A sub-constraint is set as a modifier via the syntax `cfg//:<constraint>/sub_constraint[<value>]`. The reported constraint setting label is `cfg//:<constraint>/sub_constraint`.
- A sub-constraint is invisible to everything outside its parent target's resolution. It cannot be referenced in any other constraint's attrs, any select outside the parent's conditional fields, any `constraint_impl` target, transitions, configured aliases, or platforms.
- Implementation-wise, sub-constraints will be implemented in the unified constraint rule as separate constraint targets. I haven't decided how to implement the private aspects of sub-constraints, but it will most likely be a reusable attr that you can use to mark a constraint as private to its parent's selects.

### `constraint_impl` rule

One UX problem with conditional constraint is that it requires all logic to live inline in the constraint definition. On popular constraints like cpp optimization level, this can balloon into long, complex selects that need to handle different variations across product types in the repo.

To avoid this, we will introduce a `constraint_impl` rule that lets you factor a chunk of conditional logic into its own target and reference it from `invariant`. For example, cpp optimization level could look like:

```python
# cfg//cpp/BUCK

constraint(
  name = "opt_level",
  values = ["o0", "o1", "o2", "o3"],
  default = "o3",
  invariant = select({
    "cfg//:project[foo]": "root//project_foo/cfg:opt_level",
    # plus other project-specific overrides
    "DEFAULT": select({
      "cfg//:build_mode[dev]": "o0",
      "DEFAULT": "o3",
    }),
  }),
)

# root//project_foo/cfg/BUCK

constraint_impl(
  name = "opt_level",
  constraint = "cfg//cpp:opt_level",
  default = "o0",
  invariant = select({
    "cfg//:build_mode[dev]": "o1",
    "DEFAULT": "o3",
  }),
)
```

Plugging in project foo's logic only takes one extra line in the `opt_level` constraint definition. All the project foo-specific logic for cpp optimization level lives in project foo directly, which is nicer from both UX and code ownership perspectives.

`constraint_impl` allows specifying `invariant`, `default`, and `compatibility`, but it will not support specifying `values`, so no additional values can be added by individual projects.

## Migration/Deprecation

### Conditional behavior

The unified `constraint()` rule will be the *only* supported way to express dependency relationships between constraints in the future. Once this feature ships, we'll deprecate two existing mechanisms and work to remove them:

- **Conditional modifiers**, which suffer from the problems documented in the [problems tab](problems.md#case-2-use-conditional-modifiers).
- **`select()` inside transition attrs.** Today these have to be double-resolved (pre and post transition) and the build fails if the two resolutions diverge.

### Select resolution semantics

Today, `select` picks the most "refined" key when multiple keys match. For example, if a configuration matches both linux and linux-arm64, `select` resolves to linux-arm64.

To support conditional constraints, we need to change how select resolves on multiple matches. Two proposals are on the table.

1. Change `select` to match the *first* key instead of the most refined key. This is our preferred solution. We think refinement encourages bad select definitions and is unintuitive for users.
2. Support a different definition of refinement. For example, count the number of unique constraint matches inside conditional constraint as the refinement metric. We don't like this in general and will only consider it as a temporary migration step if necessary to unblock conditional constraint before moving to #1.

## `ConfigurationInfo` and `PlatformInfo` API changes

`ConfigurationInfo` and `PlatformInfo` expose `.constraints` as a flat dict, plus `.get()`, `.insert()`, and `.pop()` that fall back to the constraint's default today. These semantics will be defined as follows after this RFC:

- `.get(constraint)` returns the resolved value, via a new provider separate from `ConstraintValueInfo`.
- `.insert(constraint, value)` writes a value. `value` continues to be `ConstraintValueInfo`.
- `.pop(constraint)` removes and returns the resolved value.
- Iteration over `.constraints` yields values (as `ConstraintValueInfo`).

As part of the unified constraint rule rollout, we'll migrate transition functions off raw `.constraints` access and remove the raw access path.
