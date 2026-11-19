# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Used like this:
#
#     transition_alias(
#         name = "anyhow-1.0.100-build-script-build-riscv64gc",
#         actual = ":anyhow-1.0.100-build-script-build",
#         incoming_transition = "prelude//rust/buildscript:buildscript_platform_transition[99]",
#     )
#
# This allows `anyhow-1.0.100-build-script-run` to take an exec dependency on
# this target, transition to the build-script-run's execution platform (which
# discards knowledge of build-script-run's target platform), and transition
# again to set a riscv64gc constraint in the execution platform.
transition_alias = rule(
    impl = lambda ctx: ctx.attrs.actual.providers,
    attrs = {
        "actual": attrs.dep(),
        "labels": attrs.list(attrs.string(), default = []),
    },
    supports_incoming_transition = True,
)

def _transitions_for_constraint_impl(ctx: AnalysisContext) -> list[Provider]:
    sub_targets = {}

    for name, constraint_value in ctx.attrs.constraint[DefaultInfo].sub_targets.items():
        sub_targets[name] = [
            TransitionInfo(
                impl = lambda platform, constraint = constraint_value[ConstraintValueInfo]: PlatformInfo(
                    label = platform.label,
                    configuration = ConfigurationInfo(
                        constraints = platform.configuration.constraints
                        | {
                            constraint.setting.label: constraint,
                        },
                        values = platform.configuration.values,
                    ),
                ),
            )
        ]

    return [DefaultInfo(sub_targets = sub_targets)]

# Take a constraint setting and turn each of its constraint values into a
# transition adding that constraint value into the platform configuration.
transitions_for_constraint = rule(
    impl = _transitions_for_constraint_impl,
    attrs = {
        "constraint": attrs.dep(),
    },
    is_configuration_rule = True,
)
