# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""
Functions for constructing ConditionalModifierInfo for constraint rules.

This module provides utilities to create ConditionalModifierInfo instances
with proper handling of execution platform modifiers. When a constraint has
execution_modifier=False, its modifier will be wrapped in a conditional that
skips it when configuring exec deps.
"""

load(":types.bzl", "ConditionalModifierInfo", "ModifierInfo", "ModifiersMatchInfo")

def _maybe_wrap_for_exec_platform(
    constraint_value: ConstraintValueInfo, execution_modifier: bool, exec_platform_marker_configuration_info: ConfigurationInfo | None
) -> ModifierInfo:
    """
    When execution_modifier=False, wrap the constraint value in a conditional
    modifier that returns None (skips) when the exec platform marker
    (ovr_config//platform/execution/constraints:execution-platform-transitioned)
    is present in the configuration. This means modifiers with
    execution_modifier=False won't apply when configuring exec deps.

    Args:
        constraint_value: The constraint value to potentially wrap.
        execution_modifier: If True, the modifier applies to exec deps, so no wrapping needed.
        exec_platform_marker_configuration_info: The ConfigurationInfo for the exec platform marker.
                                                  If None, no wrapping is done.

    Returns:
        Either the original constraint_value (if execution_modifier=True or no marker),
        or a ModifiersMatchInfo that conditionally skips on exec platforms.
    """
    if execution_modifier:
        # This modifier should apply on exec platforms, no wrapping needed
        return constraint_value

    if exec_platform_marker_configuration_info == None:
        # No marker constraint available (e.g., for the marker constraint itself)
        return constraint_value

    # Wrap in a conditional: when exec marker is present -> None (skip), otherwise -> apply
    return ModifiersMatchInfo(
        selector = [(exec_platform_marker_configuration_info, None)],  # Skip when on exec platform
        default = constraint_value,  # Apply on target platform
    )

def make_constraint_modifier_info(
    constraint_value: ConstraintValueInfo, key: TargetLabel, execution_modifier: bool, exec_platform_marker_configuration_info: ConfigurationInfo | None
) -> ConditionalModifierInfo:
    """
    Creates a ConditionalModifierInfo for a constraint value with proper
    handling of execution platform modifiers.

    When execution_modifier=False and exec_platform_marker_configuration_info is provided,
    the inner value will be wrapped in a ModifiersMatchInfo that causes the
    modifier to be skipped when configuring exec deps.

    Args:
        constraint_value: The constraint value this modifier represents.
        key: The constraint setting's target label (used as the modifier key).
        execution_modifier: Whether this modifier should apply on exec platforms.
        exec_platform_marker_configuration_info: The ConfigurationInfo for the exec platform marker.
                                                  Pass None for the marker constraint itself to avoid cycles.

    Returns:
        A ConditionalModifierInfo with the appropriate inner value.
    """
    wrapped_inner = _maybe_wrap_for_exec_platform(
        constraint_value = constraint_value,
        execution_modifier = execution_modifier,
        exec_platform_marker_configuration_info = exec_platform_marker_configuration_info,
    )

    return ConditionalModifierInfo(
        inner = wrapped_inner,
        key = key,
    )
