# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""
Apply a constraint that sets -enable-testing in Swift compilation.
"""

def enable_testing_transition_impl(platform: PlatformInfo, refs: struct) -> PlatformInfo:
    enable_testing_config = refs.enable_testing_setting[ConstraintSettingInfo].label
    enable_testing_constraint = refs.enable_testing_value[ConstraintValueInfo]
    if enable_testing_config in platform.configuration.constraints:
        # Don't apply the transition more than once
        return platform

    updated_constraints = platform.configuration.constraints
    updated_constraints[enable_testing_config] = enable_testing_constraint
    new_cfg = ConfigurationInfo(
        constraints = updated_constraints,
        values = platform.configuration.values,
    )
    return PlatformInfo(
        label = platform.label + "-enable-testing",
        configuration = new_cfg,
    )

enable_testing_transition_refs = {
    "enable_testing_setting": "config//features/apple/constraints:swift_enable_testing",
    "enable_testing_value": "config//features/apple/constraints:swift_enable_testing[enabled]",
}

enable_testing_transition = transition(
    impl = enable_testing_transition_impl,
    refs = enable_testing_transition_refs,
)
