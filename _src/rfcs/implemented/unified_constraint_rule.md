<!-- Source: https://docs.google.com/document/d/1AydiiQWBhB_VTl07jPZmyApPkjgxbs4-OQU_nC2Pt9M -->

# [RFC] Unified Constraint Rule with Defaults

## Unified Constraint Rule

### Problem

A configuration is a collection of constraints. There are two components of a constraint, constraint setting (the key) and constraint value (value for a key). Below is an example of an OS constraint with linux, macos, and windows as values.

```python
# cfg//BUCK

constraint_setting(name = "os")  # this is constraint key

# These are constraint values for the key
constraint_value(
  name = "linux",
  constraint_setting = ":os",
)

constraint_value(
  name = "macos",
  constraint_setting = ":os",
)

constraint_value(
  name = "windows",
  constraint_setting = ":os",
)
```

There are two ergonomic problems with the way that constraint values are currently defined.

1. As shown in the previous example, defining a single constraint is rather verbose.
2. It's not possible for a user to see all the possible values of a constraint setting. For example, it’s possible to define an OS constraint value in a different BUCK file from the OS constraint setting.

### Proposal

Instead, consider the following syntax for OS.

```python
# cfg//BUCK

constraint(
  name = "os",
  values = [
    "linux",
    "macos",
    "windows",
    "none",
  ],
  default = "none",
)
```

This is much more concise, and it's immediately clear to a reader that `linux`,

`macos`, and `windows` are values for `os` constraint.

The `constraint` rule creates exactly one target with label `cfg//:os`, where

`cfg//:os` is the constraint key. Values are referenced via subtargets, ex. `cfg//:os[linux]` and `cfg//:os[macos]`. These can be used in selects, for example,

```python
deps = select({
  "DEFAULT": [],
  "cfg//:os[linux]": [":linux-only-dep"],
})
```

They can also be used in modifiers or platforms.

```python
# PACKAGE file

set_cfg_modifiers(cfg_modifiers = [
  "cfg//:os[linux]",
])

# BUCK file
platform(
  name = "linux",
  constraint_values = [
    "cfg//:os[linux]",
  ],
)
```

Values must be strings.

Aliases can be used to alias subtarget values to targets for backwards compatibility with existing syntax where values are targets.

```python
# This is old syntax
constraint_value(
  name = "linux",
  constraint_setting = ":os",
)

# This is equivalent target that uses new syntax.
configuration_alias(
  name = "linux",
  actual = ":os[linux]",
)

# In some other BUCK file
deps = select({
  "DEFAULT": [],
  "cfg//:linux": [":linux-only-dep"],
})
```

The plan is to migrate all constraints to the unified constraint rule and delete older `constraint_setting` and `constraint_value` rules. We may use `configuration_alias` rule to alias subtarget constraint values to maintain backwards compatibility for popular constraint values.

## `default` attribute

### Problem

Suppose we have the following constraint defined for sanitizer.

```python
constraint(
  name = "sanitizer",
  values = [
    "asan",
    "tsan",
    "msan",
    "none",
  ],
  # ...
)
```

One issue with how constraint works in Buck today is that setting `cfg//:sanitizer[none]` in the configuration creates a different configuration than having `cfg//:sanitizer` unset. This means that changing between `[none]` and unset may cause Buck to do some amount of duplicate work and create different build action keys, even though they are semantically equivalent.

It also means that everywhere someone writes a `select` on sanitizer, they have to write it like this,

```python
select({  "cfg//:sanitizer[asan]": # ...
  "cfg//:sanitizer[tsan]": # ...
  "cfg//:sanitizer[msan]": # ...
  "DEFAULT": # ...
})
```

Where the “DEFAULT” is aligned with `cfg//:sanitizer[none]`. This puts the burden on all select writers to structure select correctly. This also means that if someone adds a new value for sanitizer (ex `ubsan`), the select would always evaluate to “DEFAULT” rather than fail when `ubsan` is set, meaning that the author introducing `ubsan` would have to grep through selects to update manually.

Additionally, the fact that default is not the same as `[none]` means that writing something like `is_sanitizer_enabled` looks as follows.

```python
select({
  "cfg//:sanitizer[asan]": True,
  "cfg//:saniitzer[tsan]": True,
  "cfg//:sanitizer[msan]": True,
  "DEFAULT": False,
})
```

Because you cannot just select on `cfg//:sanitizer[none]` to know that sanitizer is disabled.

### Proposal

The unified constraint rule enables a way to define defaults for constraints such that a configuration with default constraint value is equivalent to a configuration with the constraint unset.

```python
constraint(
    name = "sanitizer",
    values = [
        "asan",
        "tsan",
        "msan",
        "none",
    ],
    default = "none",
)
```

In this example, `none` is the default value for `:sanitizer` constraint. When

a configuration does not have `:sanitizer` set, it automatically defaults to the default. In practice, this means the following select will resolve to `None` when sanitizer is unset.

```python
sanitizer_name = select({
    ":sanitizer[none]": None,
    ":sanitizer[asan]": "ASAN",
    ":sanitizer[msan]": "MSAN",
    ":sanitizer[tsan]": "TSAN",
})
```

The select from the above example is *guaranteed to resolve and never hit an*

*unresolved select error*. This solves the issue where you cannot just select on `cfg//:sanitizer[none]`. Now `is_sanitizer_enabled` can look as follows.

```python
select({
  "cfg//:sanitizer[none]": False,
  "DEFAULT": True,
})
```

Once we migrate all constraints to the unified constraint rule, we will discourage the use of `”DEFAULT”` in selects in order to make it easier for constraint owners to correctly add new values to their constraints.

As mentioned previously, a configuration with `cfg//:sanitizer[none]` will be identical to a configuration without sanitizer constraint set.

The default must be a value listed in `values` attribute.

Default can also be used as a keyword directly by referencing `cfg//:sanitizer[default]`. In this case, `cfg//:sanitizer[default]` would be equivalent to using `cfg//:sanitizer[none]`. This can be used modifiers and transitions but it cannot be used as a select key.

Once all constraints are migrated to the unified constraint rule model, `default` **will be a required keyword **on unified constraint rule. This means that in practice, all constraints must have two or more values in order to be useful in practice.

Note select on defaults mentioned in the original RFC are still planned to be supported, but now it will be split out into a separate RFC.

## New ConfigurationInfo API

### Background

Currently in Buck2, [`ConfigurationInfo`](https://buck2.build/docs/api/build/ConfigurationInfo/) exposes `.constraints` as a dictionary attribute that can be directly accessed and manipulated. With unified constraint, a constraint setting can now declare a default constraint value that should be used when the constraint is not explicitly set.

### Problem

To support defaults properly, we need to make `ConfigurationInfo` opaque so that when a constraint is queried but not set, we will return its default value. This is required to make a configuration with constraint unset to behave the same as a configuration with default explicitly set and thus produce the same action keys.

### Proposal

Add dictionary-like APIs to `ConfigurationInfo` before making it opaque.

```python
ConfigurationInfo.get(key: ConstraintSettingInfo) -> ConstraintValueInfo | None
```

Get a constraint value by its constraint setting. If the constraint is not set, returns the default constraint value from the setting (if defined). Returns `None` only if the constraint is not set AND the setting has no default

```python
ConfigurationInfo.insert(value: ConstraintValueInfo) -> ConstraintValueInfo | None
```

Insert a constraint value into the configuration. Returns the previously set value for this constraint setting.

- **Mutates** the `ConfigurationInfo` object in-place
- Returns the previously set `ConstraintValueInfo` if one existed
- Returns the default `ConstraintValueInfo` from the constraint setting if no previous value existed but a default exists
- Returns `None` only if there was no previous value AND the setting has no default

```python
ConfigurationInfo.pop(key: ConstraintSettingInfo) -> ConstraintValueInfo | None
```

Remove and return a constraint value by its constraint setting.

- **Mutates** the `ConfigurationInfo` object in-place
- Returns the removed `ConstraintValueInfo` if it was set
- Returns the default `ConstraintValueInfo` from the constraint setting if not set but a default exists
- Returns `None` only if the constraint was not set AND the setting has no default

```python
ConfigurationInfo.copy() -> ConfigurationInfo
```

Create a copy of the `ConfigurationInfo`. It is useful in split transitions.

#### Example

**Example 1: Simple Transition**

```python
def _transition_impl(platform: PlatformInfo, ref) -> PlatformInfo:
  linux_value = ref.linux[ConstraintValueInfo]
  os_setting = linux_value.setting
  opt_value = ref.opt[ConstraintValueInfo]
  # Get current target platform OS (may return default if not set)
  configuration = platform.configuration
  current_os = configuration.get(os_setting)
  # Only transition if not Linux
  if current_os != linux_value:
    configuration.insert(opt_value)
  platform = PlatformInfo(
    label = "<transition-to-opt-on-non-linux>",
    configuration,
  )
  return platform
```

**Example 2: Split Transition**

```python
def _os_split_transition(platform: PlatformInfo) -> dict[str, PlatformInfo]:
  opt_value = ref.opt[ConstraintValueInfo]
  build_mode_setting = opt_value.setting
  macos_config = platform.configuration.copy()
  macos_config.insert(opt_value)
  linux_config = platform.configuration.copy()
  linux_config.pop(buildmode_setting)
  configs = {
    "macos": PlatformInfo(
      label = "<macos-opt>",
      configuration = macos_config,
    ),
    "linux": PlatformInfo(
      label = "<linux-default>",
      configuration = linux_config,
    )
  }
  return configs
```

#### Deprecation

After these APIs are added and we finish the migration, we will deprecate the direct `.constraints` and `.values`.

#### Alternatives Considered

Instead of mutable methods, make `ConfigurationInfo` immutable where `.insert()` and `.pop()` return a new `ConfigurationInfo` object

**Pros:**

- Immutability
- No need for explicit `.copy()` method

**Cons:**

- **Not Pythonic/Starlark-like**: Python dictionaries and Starlark conventions favor mutability
- **User confusion**: Users expect `.insert()` to mutate in-place based on Python dict experience
- **Verbose**: Would require reassignment everywhere: `config = config.insert(value)`

Link to google doc: [[RFC] Unified Constraint Rule with Defaults](https://docs.google.com/document/d/1AydiiQWBhB_VTl07jPZmyApPkjgxbs4-OQU_nC2Pt9M/edit?usp=sharing)
