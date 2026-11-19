# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""
Apply a constraint if the minimum_os_version attribute is set.
This overrides any existing target_sdk_version select.
"""

load("@prelude//apple:versions.bzl", "TARGET_SDK_VERSIONS")
load("@prelude//apple/user:enable_testing_transition.bzl", "enable_testing_transition_impl", "enable_testing_transition_refs")

def target_sdk_version_transition_impl(platform: PlatformInfo, refs: struct, attrs: struct) -> PlatformInfo:
    target_sdk_version = attrs.minimum_os_version
    if not target_sdk_version:
        return platform

    constraint_label = refs.version[ConstraintSettingInfo].label
    constraint_value = platform.configuration.constraints.get(constraint_label)
    version_provider = getattr(refs, target_sdk_version, None)
    if version_provider == None:
        fail("target sdk version {} is missing constraints".format(target_sdk_version))

    version_constraint = version_provider[ConstraintValueInfo]
    if constraint_value == version_constraint:
        return platform

    updated_constraints = platform.configuration.constraints
    updated_constraints[constraint_label] = version_constraint
    new_cfg = ConfigurationInfo(
        constraints = updated_constraints,
        values = platform.configuration.values,
    )
    return PlatformInfo(
        label = platform.label + "_target_sdk_version_" + target_sdk_version,
        configuration = new_cfg,
    )

def _target_sdk_version_and_enable_testing_impl(platform: PlatformInfo, refs: struct, attrs: struct) -> PlatformInfo:
    platform_info = target_sdk_version_transition_impl(platform, refs, attrs)
    platform_info = enable_testing_transition_impl(platform_info, refs)
    return platform_info

target_sdk_version_transition_refs = {version: "@config//version:constraint-value-target-sdk-version-" + version for version in TARGET_SDK_VERSIONS} | {
    "version": "@config//version:constraint-setting-target-sdk-version"
}

target_sdk_version_transition_attrs = ["minimum_os_version"]

target_sdk_version_transition = transition(
    impl = target_sdk_version_transition_impl,
    refs = target_sdk_version_transition_refs,
    attrs = target_sdk_version_transition_attrs,
)

# apple_test requires both enable_testing and target_sdk_version
# applied as an incoming transition, so we chain the transitions
# in this implementation.
apple_test_target_sdk_version_transition = transition(
    impl = _target_sdk_version_and_enable_testing_impl,
    refs = enable_testing_transition_refs | target_sdk_version_transition_refs,
    attrs = target_sdk_version_transition_attrs,
)
