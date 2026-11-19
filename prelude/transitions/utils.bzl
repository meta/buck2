# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _filtered_platform_constraints(platform: PlatformInfo, constraint_settings_labels_to_remove: list[TargetLabel]) -> dict[TargetLabel, ConstraintValueInfo]:
    return {
        constraint_setting_label: constraint_setting_value
        for (constraint_setting_label, constraint_setting_value) in platform.configuration.constraints.items()
        if constraint_setting_label not in constraint_settings_labels_to_remove
    }

def _get_constraint_value(platform: PlatformInfo, constraint: ConstraintSettingInfo) -> [None, ConstraintValueInfo]:
    return platform.configuration.constraints.get(constraint.label)

def _modify_constraints_transition_impl(ctx: AnalysisContext) -> list[Provider]:
    add = [dep[ConstraintValueInfo] for dep in ctx.attrs.add]
    remove = [dep[ConstraintValueInfo] for dep in ctx.attrs.remove]

    def tr_impl(platform: PlatformInfo) -> PlatformInfo:
        new_constraints = dict(platform.configuration.constraints)

        for constraint in remove:
            new_constraints.pop(constraint.setting.label, None)
        for constraint in add:
            new_constraints[constraint.setting.label] = constraint

        return PlatformInfo(
            label = platform.label,
            configuration = ConfigurationInfo(
                constraints = new_constraints,
                values = platform.configuration.values,
            ),
        )

    return [
        DefaultInfo(),
        TransitionInfo(
            impl = tr_impl,
        ),
    ]

_modify_constraints_transition = rule(
    impl = _modify_constraints_transition_impl,
    attrs = {
        "add": attrs.list(attrs.dep(), default = []),
        "remove": attrs.list(attrs.dep(), default = []),
    },
    is_configuration_rule = True,
)

transition_utils = struct(
    filtered_platform_constraints = _filtered_platform_constraints,
    get_constraint_value = _get_constraint_value,
    modify_constraints_transition = _modify_constraints_transition,
)
