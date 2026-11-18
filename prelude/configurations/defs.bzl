# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//cfg/exec_platform:marker.bzl", "get_exec_platform_marker")
load("@prelude//cfg/modifier:constraint_modifier_info.bzl", "make_constraint_modifier_info")
load("@prelude//cfg/modifier:types.bzl", "ConditionalModifierInfo")
load(":util.bzl", "util")

_ExecutionModifierInfo = provider(
    fields = {
        "execution_modifier": bool,
    }
)

def config_setting_impl(ctx):
    subinfos = [util.constraint_values_to_configuration(ctx.attrs.constraint_values)]
    subinfos.append(
        util.configuration_info(
            constraints = {},
            values = ctx.attrs.values,
            root_values = ctx.attrs.root_values,
        )
    )
    cfg_info = util.configuration_info_union(subinfos)
    providers = [DefaultInfo(), cfg_info]
    if len(ctx.attrs.constraint_values) == 1:
        # When only have one constraint, we return an additional ConditionalModifierInfo provider for
        # conditional modifier
        conditional_modifier_info = ctx.attrs.constraint_values[0][ConditionalModifierInfo]
        providers.append(conditional_modifier_info)
    return providers

def constraint_setting_impl(ctx):
    return [
        DefaultInfo(),
        ConstraintSettingInfo(label = ctx.label.raw_target()),
        # In order for the constraint_value to access the execution modifier info, we need to provide an additional provider here.
        _ExecutionModifierInfo(execution_modifier = ctx.attrs.execution_modifier),
    ]

def constraint_value_impl(ctx):
    constraint_value = ConstraintValueInfo(
        setting = ctx.attrs.constraint_setting[ConstraintSettingInfo],
        label = ctx.label.raw_target(),
    )
    execution_modifier_info = ctx.attrs.constraint_setting[_ExecutionModifierInfo]

    return [
        DefaultInfo(),
        constraint_value,
        # Provide `ConfigurationInfo` from `constraint_value` so it could be used as select key.
        ConfigurationInfo(
            constraints = {
                constraint_value.setting.label: constraint_value,
            },
            values = {},
        ),
        make_constraint_modifier_info(
            constraint_value = constraint_value,
            key = constraint_value.setting.label,
            execution_modifier = execution_modifier_info.execution_modifier,
            exec_platform_marker_configuration_info = ctx.attrs._exec_platform_marker[ConfigurationInfo],
        ),
    ]

def constraint_impl(ctx):
    # Validate values are unique and non-empty
    values = ctx.attrs.values

    min_values = 1 if ctx.attrs.allow_trivial_constraint else 2
    if len(values) < min_values:
        if ctx.attrs.allow_trivial_constraint:
            fail("constraint() rule must have at least one value.")
        else:
            fail(
                "constraint() rule must have at least two values. "
                + "A single-value constraint has no selectivity: it can't express a choice between alternatives "
                + "(e.g., 'linux' vs 'macos', 'enabled' vs 'disabled'), which is the whole point of a constraint. "
                + "If you don't need to select between alternatives, you likely don't need a constraint at all.\n\n"
                + "Set allow_trivial_constraint = True only if your constraint legitimately has just one value "
                + "most of the time and may temporarily gain alternatives (e.g., versioned packages "
                + "that pick up an extra value during an upgrade and drop it once the upgrade lands).",
            )

    default = ctx.attrs.default

    # Value-name reservations:
    # - 'DEFAULT' is always reserved, to avoid visual confusion with the special `[default]` subtarget.
    # - 'default' is allowed only when it is itself the default value, in which case it coincides with the
    #   `[default]` subtarget added below. Otherwise `[default]` would be ambiguous, so it is rejected.
    seen = set()
    for v in values:
        if v == "DEFAULT":
            fail("'DEFAULT' is a reserved keyword and cannot be used as a constraint value. Use a different name to avoid confusion.")
        if v == "default" and default != "default":
            fail(
                "'default' can be used as a constraint value only when it is also the default value "
                + "(i.e. default = \"default\"); otherwise ':{}[default]' is ambiguous between the value ".format(ctx.label.name)
                + "named 'default' and the default value '{}'.".format(default),
            )
        if v in seen:
            fail("Duplicate value '{}' in constraint()".format(v))
        seen.add(v)

    # Validate default is one of the declared values.
    if default not in seen:
        fail("default value '{}' must be one of the declared values: {}".format(default, values))

    # Main target provides the constraint setting
    main_label = ctx.label.raw_target()
    constraint_setting = ConstraintSettingInfo(
        label = main_label,
        default = main_label.with_sub_target(default),
    )

    # exec_platform_marker_constraint() and constraint() rule both use this impl,
    # but only constraint() has this attribute.
    exec_platform_marker_configuration_info = None
    if hasattr(ctx.attrs, "_exec_platform_marker"):
        exec_platform_marker_configuration_info = ctx.attrs._exec_platform_marker[ConfigurationInfo]

    execution_modifier = False
    if hasattr(ctx.attrs, "execution_modifier"):
        execution_modifier = ctx.attrs.execution_modifier

    # Create subtargets for each value
    sub_targets = {}
    for value_name in values:
        # Create a constraint value for this subtarget
        constraint_value = ConstraintValueInfo(
            setting = constraint_setting,
            label = main_label.with_sub_target(value_name),
        )

        sub_targets[value_name] = [
            DefaultInfo(),
            constraint_value,
            ConfigurationInfo(
                constraints = {
                    main_label: constraint_value,
                },
                values = {},
            ),
            make_constraint_modifier_info(
                constraint_value = constraint_value,
                key = main_label,
                execution_modifier = execution_modifier,
                exec_platform_marker_configuration_info = exec_platform_marker_configuration_info,
            ),
        ]

    # Add 'default' subtarget that aliases to the actual default value
    sub_targets["default"] = sub_targets[default]

    # Validate and register user-defined aliases.
    # Each alias becomes an additional subtarget pointing to an existing value's providers.
    reserved_keywords = ["default", "DEFAULT"]
    aliases = ctx.attrs.aliases
    for alias_name, alias_value in aliases.items():
        if alias_name in reserved_keywords:
            fail("alias '{}' is a reserved keyword and cannot be used as a constraint alias.".format(alias_name))
        if alias_name in seen:
            fail("alias '{}' conflicts with a declared constraint value.".format(alias_name))
        if alias_value not in seen:
            fail("alias '{}' targets value '{}' which is not declared in values: {}".format(alias_name, alias_value, values))
        sub_targets[alias_name] = sub_targets[alias_value]

    return [
        DefaultInfo(sub_targets = sub_targets),
        constraint_setting,
        _ExecutionModifierInfo(execution_modifier = execution_modifier),
    ]

def platform_impl(ctx):
    subinfos = [dep[PlatformInfo].configuration for dep in ctx.attrs.deps] + [util.constraint_values_to_configuration(ctx.attrs.constraint_values)]
    return [
        DefaultInfo(),
        PlatformInfo(
            label = str(ctx.label.raw_target()),
            # TODO(nga): current behavior is the last constraint value for constraint setting wins.
            #   This allows overriding constraint values from dependencies, and moreover,
            #   it allows overriding constraint values from constraint values listed
            #   in the same `constraint_values` attribute earlier.
            #   If this is intentional, state it explicitly.
            #   Otherwise, fix it.
            configuration = util.configuration_info_union(subinfos),
        ),
    ]

def configuration_alias_impl(ctx: AnalysisContext) -> list[Provider]:
    return ctx.attrs.actual.providers

config_setting = prelude_rule(
    name = "config_setting",
    impl = config_setting_impl,
    is_configuration_rule = True,
    docs = """
        `config_setting()` accepts a list of `constraint_values` and a list of values
        (buckconfig keys + expected values) and matches if all of those match.

        This is implemented as forming a single `ConfigurationInfo` from the union of the
        referenced values and the config keys.
    """,
    examples = None,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "constraint_values": attrs.list(attrs.configuration_label(), default = []),
            "values": attrs.dict(
                key = attrs.string(),
                value = attrs.string(),
                sorted = False,
                default = {},
                doc = """
                    Buckconfig key -> expected value, evaluated from the target's cell (like `read_config`).
                    A given `section.key` may appear in either `values` or `root_values`, but not both.
                """,
            ),
            "root_values": attrs.dict(
                key = attrs.string(),
                value = attrs.string(),
                sorted = False,
                default = {},
                doc = """
                    Buckconfig key -> expected value, evaluated from the root cell (like `read_root_config`).
                    A given `section.key` may appear in either `values` or `root_values`, but not both.
                """,
            ),
        }
    ),
)

configuration_alias = prelude_rule(
    name = "configuration_alias",
    impl = configuration_alias_impl,
    is_configuration_rule = True,
    docs = """
        `configuration_alias()` acts like `alias()` but for configuration targets.

        The `configuration_alias` itself is a configuration rule and the `actual` attribute
        is expected to be a target defined by a configuration rule (such as `constraint`,
        `constraint_setting`, `constraint_value`, `config_setting`, or `platform`).

        Unlike regular `alias()`, `configuration_alias()` can be used wherever configuration
        targets are expected, such as in `select()` keys or `platform.constraint_values`.

        This rule is particularly useful for creating backwards-compatible aliases to constraint
        values defined using the unified `constraint()` rule, where values are referenced via
        subtargets (e.g., `:os[linux]`).
    """,
    examples = """
        ```
        # Define a constraint with multiple values using the unified constraint rule
        constraint(
            name = "os",
            values = ["linux", "macos", "windows", "none"],
            default = "none",
        )

        configuration_alias(
            name = "linux",
            actual = ":os[linux]",
        )

        configuration_alias(
            name = "macos",
            actual = ":os[macos]",
        )

        # The alias can be used in platform definitions
        platform(
            name = "linux_platform",
            constraint_values = [
                ":linux",  # Using the alias instead of :os[linux]
            ],
        )

        # The alias can be used in select() expressions
        genrule(
            name = "my_rule",
            cmd = select({
                ":linux": "echo linux",
                ":macos": "echo macos",
                ":os[none]": "echo other",  # Can also use subtarget directly
            }),
            out = "out.txt",
        )
        ```
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "actual": attrs.dep(
                pulls_and_pushes_plugins = plugins.All,
                doc = """
                The target to alias. This should be a target defined by a configuration rule
                such as `constraint`, `constraint_setting`, `constraint_value`, `config_setting`,
                or `platform`.
            """,
            ),
        }
    ),
)

constraint = prelude_rule(
    name = "constraint",
    impl = constraint_impl,
    is_configuration_rule = True,
    docs = """
        Unified constraint rule that defines both a constraint setting and its possible values.
        Values are exposed as subtargets, e.g., `cfg//:os[linux]`.
    """,
    examples = """
        ```
        constraint(
            name = "os",
            values = ["linux", "macos", "windows"],
            default = "linux",
        )
        ```
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "values": attrs.list(
                attrs.string(),
                doc = "List of value names.",
            ),
            "default": attrs.string(
                doc = "Default value (must be one of the `values`).",
            ),
            "allow_trivial_constraint": attrs.bool(
                default = False,
                doc = """
                    If True, allow this constraint to declare only one value.
                    Intended for constraints that have just a default value most of the time and may
                    temporarily gain additional values (e.g., versioned packages that pick up an extra
                    value during an upgrade and drop it once the upgrade lands).
                    Default is False, which requires at least two values.
                """,
            ),
            "aliases": attrs.dict(
                attrs.string(),
                attrs.string(),
                default = {},
                doc = """
                    Optional mapping of alias name -> declared value. Each alias becomes an additional
                    subtarget that resolves to the same providers as the underlying value, letting users
                    refer to a constraint value by a stable shorthand (e.g. `:version[1.4]`) that maps to
                    a brittle underlying value (e.g. `1.4.5.xxx.67890`). Alias names must not collide with
                    declared values or reserved keywords, and alias values must appear in `values`.
                """,
            ),
            "execution_modifier": attrs.bool(default = False),
            # Dependency on the exec platform marker constraint.
            # Modifiers with execution_modifier=False will be wrapped in a conditional modifier
            # that skips them on exec platforms.
            "_exec_platform_marker": attrs.dep(providers = [ConfigurationInfo], default = get_exec_platform_marker()),
        }
    ),
)

constraint_setting = prelude_rule(
    name = "constraint_setting",
    impl = constraint_setting_impl,
    is_configuration_rule = True,
    docs = "Deprecated: use `constraint()` instead. Declares the existence of a constraint, whose values are defined using `constraint_value()`.",
    examples = None,
    further = None,
    attrs = (
        {
            "execution_modifier": attrs.bool(default = False),
        }
    ),
)

constraint_value = prelude_rule(
    name = "constraint_value",
    impl = constraint_value_impl,
    is_configuration_rule = True,
    docs = "Deprecated: use `constraint()` instead. Declares a specific value of a `constraint_setting`.",
    examples = None,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "constraint_setting": attrs.configuration_label(
                doc = "The constraint setting this value is attached to.",
            ),
            # Dependency on the exec platform marker constraint.
            # Modifiers with execution_modifier=False will be wrapped in a conditional modifier
            # that skips them on exec platforms.
            "_exec_platform_marker": attrs.dep(providers = [ConfigurationInfo], default = get_exec_platform_marker()),
        }
    ),
)

exec_platform_marker_constraint = prelude_rule(
    name = "exec_platform_marker_constraint",
    impl = constraint_impl,
    is_configuration_rule = True,
    docs = """
        Special constraint rule for defining execution platform markers.
        This rule has the same semantics as `constraint()` but exists to avoid
        circular dependency issues when other constraint/constraint_value rules
        need to depend on the exec platform marker.
        Should only be defined once in a repo.
    """,
    examples = """
        exec_platform_marker_constraint(
            name = "is_exec_platform",
            values = ["true", "false"],
            default = "false",
        )
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "values": attrs.list(
                attrs.string(),
                doc = "List of value names.",
            ),
            "default": attrs.string(
                doc = "Default value (must be one of the `values`).",
            ),
            "allow_trivial_constraint": attrs.bool(default = False),
            "aliases": attrs.dict(attrs.string(), attrs.string(), default = {}),
        }
    ),
)

platform = prelude_rule(
    name = "platform",
    impl = platform_impl,
    is_configuration_rule = True,
    docs = "Declares a platform, which is a build configuration composed of constraint values.",
    examples = None,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "deps": attrs.list(
                attrs.configuration_label(),
                default = [],
                doc = "List of other platform target dependencies. The constraints from these platforms will be part of this platform (unless overridden)",
            ),
        }
        | {
            "constraint_values": attrs.list(
                attrs.dep(providers = [ConstraintValueInfo]),
                default = [],
                doc = "List of constraint values that are set for this platform.",
            ),
        }
    ),
)
