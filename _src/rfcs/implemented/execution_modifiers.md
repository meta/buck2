<!-- Source: https://docs.google.com/document/d/1fdzZA3U_uDSuB1kHDScOqfoh5lXK2lVpLyabucWMLHY -->

# RFC: Execution Modifiers

## Context

The execution configuration determines the configuration for execution deps (build tools like compilers and linkers), as opposed to the target platform for the final output. Every configured target node gets assigned an exec configuration at configuration time in a process known as “execution configuration resolution.”

The current process works as follows:

1. Start with a global priority-ordered list of execution platforms.
2. Iterate through the list and configure all exec deps against an execution platform
3. Select the first platform that satisfies two key constraints:
  - The target's `exec_compatible_with` are compatible with the exec platform
  - Each `target_compatible_with` of exec dep is compatible with that dep’s configuration.

Unfortunately, we have many problems with execution configuration resolution today:

- **Targets get completely different configurations when used as exec deps than as top-level configuration**. This results in unintuitive behavior for users when a modifier used only affects a binary’s target configuration but not its exec configuration when used as an exec dep.
- **Inability to do incremental rollout of constraints for exec deps**. A consequence of the previous point. Modifiers can be used for incremental rollout, but not for exec deps.
- **Cannot enable complex exec dep behavior needed for certain integrations**. For example, our default linux exec platforms today require the use of a specific toolchain version and that comes into conflict with certain cross-platform targets that require the use of different toolchains, even when used as exec deps. We want these targets to use different toolchains without introducing more exec platforms.
- **Combinatorial explosion**: Today, we need to manually define execution platforms for all combinations that exec deps could use. We have hundreds of exec platforms today when we only have tens of different RE environments where different builds can execute on.

## Proposed Solution: Execution Platform Modifiers

In this new model, we will apply PACKAGE-level and target-level modifiers set directly on target definitions of exec deps as part of the configuration process for exec deps.

Specifically, when configuring each exec dep, instead of just configuring it against an exec platform, we will obtain a configuration by resolving modifiers from multiple sources.

## Modifier Sources and Resolution Order

When configuring an exec dep `E`, modifiers are applied from the following sources in priority order (later sources override earlier ones):

- **Package-level modifiers of `E`**: Set via `set_modifiers()` in PACKAGE files of `E`
- **Target-level modifiers of `E`**: Set directly on the exec dep target definition `foo(..., exec_dep = ":tool", ...) some_tool(name = "tool", modifiers = ["cfg//:compiler[clang]"])`
- **Execution platform modifiers:** Constraints defined on the global execution platform are treated as modifiers and have the highest priority.

This ordering means that execution platform modifiers will override target-level and PACKAGE-level modifiers of the same constraint.

## Execution Platform Selection

When resolving the execution platform, we will still pick the first platform that satisfies all compatibility requirements, with one important change:

- **`target_compatible_with`** of each exec dep is evaluated against the configuration of each exec dep **after exec modifier resolution**
- **`exec_compatible_with`** evaluation remains unchanged and evaluates directly against the exec platform

## Only applying modifiers to target or exec platform

Certain modifiers are intended to be only applied to target or exec platforms and not both, even when they are applied at a repo-level. For example, if you were to specify that you want to use ASAN by default on linux, you don’t want that to apply to your build tools as well.

For these cases, you would need to use conditional modifiers to opt out of applying these modifiers onto the exec configuration, by matching on some type of exec-platform-transitioned constraint.

```python
conditional_modifier(
  name = asan_on_linux,
  modifier = modifiers.conditional({
    "cfg//:exec-platform[true]": None,
    "cfg//:exec-platform[false]": modifiers.conditional({
      "cfg//:os[linux]": "cfg//:sanitizer[asan]",
      "DEFAULT": None,
    }),
  }),)

# Now you can set asan_on_linux modifier on a target and it would not affect how it gets used as an exec dep.
```

In the future, we expect most of these to be defined directly on constraints via some type of [select on default API](https://docs.google.com/document/d/1AydiiQWBhB_VTl07jPZmyApPkjgxbs4-OQU_nC2Pt9M/edit?tab=t.0#bookmark=id.29ksul4o76j6), once that’s available.

## Constraint Opt-in Mechanism

To control which PACKAGE-level/target-level modifiers can be applied on the exec platform, all `constraint_setting` definitions will support an `execution_modifier `field:

- **Default**: `execution_modifier = False` (for safe rollout)
- **Eventual goal**: Constraints have `execution_modifier = True` by default. Only select constraints will need to `execution_modifier = False `to opt out of exec modifier resolution

TODO(scottcao): maybe this should just be implemented implicitly as conditional modifiers on exec platform transitioned constraint value as well.

## Execution Platform Simplification

Execution platform modifiers will be identical to current execution platform definitions today to start with. The long-term goal is to simplify these definitions significantly.

### Example:

**Today:**

The base linux x86 platform currently has constraints for:

- fbcode toolchain versions
- compiler versions
- OS and architecture

**Future state:**

This platform would only specify:

- Linux
- x86_64

All other constraints (toolchain versions, compiler versions, etc.) would be defined on the exec deps themselves as PACKAGE or target-level modifiers.

We expect that we will have a roughly 1:1 mapping between exec platforms and Remote Execution platforms, instead of the current many-to-one mapping.

## Configuration Constructor Changes

To support execution platform modifiers, we will modify the `cfg_constructor` to include an `configuring_exec_dep` flag:

```python
def cfg_constructor_pre_constraint_analysis(
    legacy_platform: PlatformInfo | None,
    package_modifiers: dict[str, Any] | None,
    target_modifiers: dict[str, Any] | None,
    cli_modifiers: list[str],
    rule_name: str,
    aliases: struct | None,
    extra_data: dict[str, Any] | None,
    configuring_exec_dep: bool = False,  # NEW: indicates whether this is an exec dep
) -> (list[str], ConfigurationParams):
    # When configuring_exec_dep=True, the cfg_constructor can apply exec-specific
    # modifier resolution logic
    ...
```

This flag allows the configuration constructor to distinguish between regular targets and exec deps, enabling different modifier resolution strategies.

## **Example**

```python
cxx_binary(
    name = "my_service",
    srcs = ["main.cpp"],
    exec_deps = [":my_clang_compiler"],
    default_platform = "root//platform/macos:arm64",
    modifiers = [
        "root//cfg:build_mode[dev]",
    ],
)

# tool as an exec deps
cxx_binary(
    name = "my_clang_compiler",
    modifiers = [
        "root//cfg:build_mode[opt]",
        "root//cfg:compiler[clang-15]",
    ],
)
```

### **Configuration Details**

**Package-level modifier** for `my_clang_compiler`: `root//cfg:sanitizer[asan]`

**Execution platform resolution**: `fbcode//buck2/platform/execution:linux-x86_64-nosan`

### **Behavior Change**

**Before this RFC:**

- Modifiers on the exec dep target (`my_clang_compiler`) were **ignored**
  - Target-level: `root///cfg:build_mode[opt]`, `root//cfg:compiler[clang-15]`
  - Package-level: `root//cfg:sanitizer[asan]`
- The exec dep inherited the main target's configuration

**After this RFC:**

- Modifiers on the exec dep target (`my_clang_compiler`) are **applied** to the execution platform configuration
  - Gets: `build_mode=opt`, `compiler=clang-15`
- Modifier `root//cfg:sanitizer[asan]` do not apply to exec dep target, because it is superceded by `root//cfg:sanitizer[nosan]` from exec platform itself.
- Modifiers on the main target (`my_service`) do **NOT** affect the exec dep
  - The `root//cfg:build_mode[dev]` only applies to `my_service` itself

## Future steps after this RFC

Once this RFC is implemented and stabilized, we will explore **target-to-exec modifiers** - a feature that propagates modifiers from the target platform to the exec platform.

This is an advanced feature that builds upon this RFC. Note that this should only be used in niche circumstances and we think this should only be used for cases where you need to build your own compilers and compiler tools from source. If you have other cases, they should likely be solved in other ways.
