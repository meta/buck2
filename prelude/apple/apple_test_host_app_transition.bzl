# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _apple_test_host_app_transition_impl(platform: PlatformInfo, refs: struct, attrs: struct) -> PlatformInfo:
    if not attrs.embed_xctest_frameworks_in_test_host_app:
        return platform

    updated_constraints = dict(platform.configuration.constraints)

    test_host_marker_setting_label = refs.embed_xctest_frameworks_constraint_setting[ConstraintSettingInfo].label
    if test_host_marker_setting_label in updated_constraints:
        return platform

    test_host_marker_value_info = refs.embed_xctest_frameworks_marker_constraint_value[ConstraintValueInfo]
    updated_constraints[test_host_marker_setting_label] = test_host_marker_value_info

    return PlatformInfo(
        label = platform.label + "-test-host-app",
        configuration = ConfigurationInfo(
            constraints = updated_constraints,
            values = platform.configuration.values,
        ),
    )

apple_test_host_app_transition = transition(
    impl = _apple_test_host_app_transition_impl,
    refs = {
        "embed_xctest_frameworks_constraint_setting": "config//marker/apple/constraints:embed_xctest_frameworks",
        "embed_xctest_frameworks_marker_constraint_value": "config//marker/apple/constraints:embed_xctest_frameworks[enabled]",
    },
    attrs = [
        "embed_xctest_frameworks_in_test_host_app",
    ],
)
