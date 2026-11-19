# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""
Transition from iOS to watchOS. Used for watchOS bundle rule.
Transforms both OS and SDK constraints.
Only sanity check for source configuration is done.
"""

load("@prelude//transitions:utils.bzl", "transition_utils")
load("@prelude//utils:expect.bzl", "expect")

def _watch_transition_impl(platform: PlatformInfo, refs: struct) -> PlatformInfo:
    # This functions operates in the following way:
    #  - Start with all the constraints from the platform and filter out the constraints for OS and SDK.
    #  - If the old OS constraint was iOS or watchOS, set the new constraint to be always watchOS.
    #  - If the old SDK constraint was iOS, replace with the equivalent watchOS constraint.
    #  - Return a new platform with the updated constraints.
    constraint_settings_to_remove = [
        refs.os[ConstraintSettingInfo].label,
        refs.sdk[ConstraintSettingInfo].label,
    ]
    # @oss-disable[end= ]: constraint_settings_to_remove.append(refs.memory_profiler[ConstraintValueInfo].setting.label)
    updated_constraints = transition_utils.filtered_platform_constraints(platform, constraint_settings_to_remove)

    # Update OS constraint
    old_os = transition_utils.get_constraint_value(platform, refs.os[ConstraintSettingInfo])
    watchos = refs.watchos[ConstraintValueInfo]
    ios = refs.ios[ConstraintValueInfo]
    if old_os != None:
        expect(
            old_os.label in [watchos.label, ios.label],
            "If present, OS transitioned non-identically to watchOS should be `iphoneos`, got {}".format(old_os.label),
        )
    updated_constraints[refs.os[ConstraintSettingInfo].label] = watchos

    # Update SDK constraint
    old_sdk = transition_utils.get_constraint_value(platform, refs.sdk[ConstraintSettingInfo])
    watchos_device_sdk = refs.watchos_device_sdk[ConstraintValueInfo]
    watchos_simulator_sdk = refs.watchos_simulator_sdk[ConstraintValueInfo]
    ios_device_sdk = refs.ios_device_sdk[ConstraintValueInfo]
    ios_simulator_sdk = refs.ios_simulator_sdk[ConstraintValueInfo]
    is_simulator = True
    if old_sdk != None:
        if old_sdk.label == watchos_simulator_sdk.label:
            pass
        elif old_sdk.label == watchos_device_sdk.label:
            is_simulator = False
        elif old_sdk.label == ios_simulator_sdk.label:
            pass
        elif old_sdk.label == ios_device_sdk.label:
            is_simulator = False
        else:
            fail("If present, SDK transitioned non-identically to watchOS should be either `iphoneos` or `iphonesimulator`, got {}".format(old_sdk.label))
    updated_constraints[refs.sdk[ConstraintSettingInfo].label] = watchos_simulator_sdk if is_simulator else watchos_device_sdk

    new_cfg = ConfigurationInfo(
        constraints = updated_constraints,
        values = platform.configuration.values,
    )
    return PlatformInfo(
        label = "watch_transition",
        configuration = new_cfg,
    )

watch_transition = transition(
    impl = _watch_transition_impl,
    refs = {
        "ios": "config//os/constraints:iphoneos",
        "ios_device_sdk": "config//os/sdk/apple/constraints:iphoneos",
        "ios_simulator_sdk": "config//os/sdk/apple/constraints:iphonesimulator",
        # @oss-disable[end= ]: "memory_profiler": "config//build_mode/apple/constraints:enable-memory-profiler-constraint-value",
        "os": "config//os/constraints:os",
        "sdk": "config//os/sdk/apple/constraints:_",
        "watchos": "config//os/constraints:watchos",
        "watchos_device_sdk": "config//os/sdk/apple/constraints:watchos",
        "watchos_simulator_sdk": "config//os/sdk/apple/constraints:watchsimulator",
    },
)
