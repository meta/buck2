# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:native.bzl", "native")

"""
Macros to help migrate from constraint_setting/constraint_value to the unified constraint() rule.

Usage:

    load("@prelude//configurations:constraint_alias.bzl", "constraint_with_aliases")

    # Simple case: all values get aliases with the same name
    constraint_with_aliases(
        name = "os",
        values = ["linux", "macos", "windows", "none"],
        default = "none",
        visibility = ["PUBLIC"],
    )
    # Creates:
    #   - constraint(name = "os", values = [...], default = "none")
    #   - configuration_alias(name = "linux", actual = ":os[linux]")
    #   - configuration_alias(name = "macos", actual = ":os[macos]")
    #   - etc.

    # Custom aliases: map old target names to new subtarget values
    constraint_with_aliases(
        name = "cgo_enabled",
        values = ["true", "false"],
        default = "false",
        aliases = {
            "cgo_enabled_true": "true",
            "cgo_enabled_false": "false",
        },
        visibility = ["PUBLIC"],
    )
    # Creates:
    #   - constraint(name = "cgo_enabled", values = ["true", "false"], default = "false")
    #   - configuration_alias(name = "cgo_enabled_true", actual = ":cgo_enabled[true]")
    #   - configuration_alias(name = "cgo_enabled_false", actual = ":cgo_enabled[false]")
"""

def constraint_with_aliases(
    name: str, values: list[str], default: str, visibility: list[str] = ["PUBLIC"], aliases: dict[str, str] | None = None, execution_modifier: bool = False
):
    """
    Creates a unified constraint rule with backwards-compatible configuration aliases.

    Args:
        name: The name of the constraint setting
        values: List of constraint values
        default: The default value (must be in values)
        visibility: Visibility for the constraint and aliases
        aliases: Optional mapping of alias target names to value names.
                 If None, creates aliases for all values using the value name as the alias name.
                 Example: {"old_name": "new_value"} creates alias "old_name" pointing to ":name[new_value]"
        execution_modifier: If True, enables exec modifier resolution for this constraint.
    """
    native.constraint(
        name = name,
        values = values,
        default = default,
        visibility = visibility,
        execution_modifier = execution_modifier,
    )

    if aliases == None:
        # Default: create aliases for all values with the same name
        for value in values:
            native.configuration_alias(
                name = value,
                actual = ":{}[{}]".format(name, value),
                visibility = visibility,
            )
    else:
        # Custom aliases: use the provided mapping
        for alias_name, value in aliases.items():
            native.configuration_alias(
                name = alias_name,
                actual = ":{}[{}]".format(name, value),
                visibility = visibility,
            )

def generate_constraint_aliases(constraint_name: str, aliases: dict[str, str] | list[str], visibility: list[str] = ["PUBLIC"]):
    """
    Generates configuration aliases for an existing constraint.

    Args:
        constraint_name: The name of the constraint setting (without leading colon)
        aliases: Either a list of value names (aliases will have the same name),
                 or a dict mapping alias names to value names.
        visibility: Visibility for the aliases
    """
    if type(aliases) == type([]):
        # List of values: alias name equals value name
        for value in aliases:
            native.configuration_alias(
                name = value,
                actual = ":{}[{}]".format(constraint_name, value),
                visibility = visibility,
            )
    else:
        # Dict mapping alias names to values
        for alias_name, value in aliases.items():
            native.configuration_alias(
                name = alias_name,
                actual = ":{}[{}]".format(constraint_name, value),
                visibility = visibility,
            )
