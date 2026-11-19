# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//apple:apple_target_sdk_version.bzl", "get_target_sdk_version_map")
load("@prelude//transitions:utils.bzl", "transition_utils")
load("@prelude//utils:expect.bzl", "expect")

"""
Transition from catalyst SDK to macOS SDK. Used for AppKit extension bundle rules.
Transforms both OS and SDK constraints.
Only sanity check for source configuration is done.
"""

def _macos_transition_impl(platform: PlatformInfo, refs: struct) -> PlatformInfo:
    # This functions operates in the following way:
    #  - Start with all the constraints from the platform and filter out the constraints for OS and SDK.
    #  - Always set the new OS constraint to macOS.
    #  - If the old SDK constraint was macCatalyst, replace with the equivalent macOS constraint.
    #  - Return a new platform with the updated constraints.
    constraint_os = refs.os[ConstraintSettingInfo]
    is_os_macos = platform.configuration.constraints[constraint_os.label] == refs.os_macos[ConstraintValueInfo]
    constraint_sdk = refs.sdk[ConstraintSettingInfo]
    is_sdk_macos = platform.configuration.constraints.get(constraint_sdk.label) == refs.sdk_macos[ConstraintValueInfo]
    if is_os_macos and is_sdk_macos:
        # Don't apply the transition more than once
        return platform

    updated_constraints = transition_utils.filtered_platform_constraints(platform, [constraint_os.label, constraint_sdk.label])

    # Update OS constraint
    updated_constraints[constraint_os.label] = refs.os_macos[ConstraintValueInfo]

    # Update SDK constraint
    old_sdk = transition_utils.get_constraint_value(platform, constraint_sdk)
    maccatalyst_sdk = refs.sdk_maccatalyst[ConstraintValueInfo]

    if old_sdk != None:
        expect(
            old_sdk.label == maccatalyst_sdk.label,
            "If present, SDK transitioned non-identically to macOS should be `macCatalyst`, got {}".format(old_sdk.label),
        )
    updated_constraints[constraint_sdk.label] = refs.sdk_macos[ConstraintValueInfo]

    constraint_target_sdk_version = refs.target_sdk_version[ConstraintSettingInfo]
    old_target_sdk_version = transition_utils.get_constraint_value(platform, constraint_target_sdk_version)
    if old_target_sdk_version != None:
        # Get last part of constraint as macCatalyst version
        maccatalyst_version = str(old_target_sdk_version.label).split("-")[-1]

        # Replace target-sdk-version constraint with macOS equivalent from mapping
        updated_constraints[constraint_target_sdk_version.label] = getattr(refs, maccatalyst_version)[ConstraintValueInfo]

    new_cfg = ConfigurationInfo(
        constraints = updated_constraints,
        values = platform.configuration.values,
    )
    return PlatformInfo(
        label = "macos_transition",
        configuration = new_cfg,
    )

refs = {
    "os": "config//os/constraints:os",
    "os_macos": "config//os/constraints:macos",
    "sdk": "config//os/sdk/apple/constraints:_",
    "sdk_maccatalyst": "config//os/sdk/apple/constraints:maccatalyst",
    "sdk_macos": "config//os/sdk/apple/constraints:macosx",
    "target_sdk_version": "config//version:constraint-setting-target-sdk-version",
}
refs.update(get_target_sdk_version_map())
macos_transition = transition(impl = _macos_transition_impl, refs = refs)
