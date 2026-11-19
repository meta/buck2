# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    "@prelude//platforms/apple:build_mode.bzl",
    "APPLE_BUILD_MODES",
    "CONSTRAINT_PACKAGE", # @oss-enable
    "get_build_mode",
)
load(
    "@prelude//platforms/apple:constants.bzl",
    "appletv_platforms",
    "ios_platforms",
    "mac_catalyst_platforms",
    "mac_platforms",
    "watch_platforms",
)
# @oss-disable[end= ]: load("@prelude//platforms/apple/meta_only:build_mode.bzl", _get_build_mode_constraints_map = "get_build_mode_constraints_map")

def get_build_mode_constraints_map(use_whatsapp_build_modes):
    return {build_mode: ["{}:{}".format(CONSTRAINT_PACKAGE, build_mode)] for build_mode in APPLE_BUILD_MODES} # @oss-enable
    # @oss-disable[end= ]: return _get_build_mode_constraints_map(use_whatsapp_build_modes)

_MOBILE_PLATFORMS = [
    appletv_platforms.APPLETVOS_ARM64,
    appletv_platforms.APPLETVSIMULATOR_ARM64,
    ios_platforms.IPHONEOS_ARM64,
    ios_platforms.IPHONEOS_ARM64E,
    ios_platforms.IPHONESIMULATOR_ARM64,
    ios_platforms.IPHONESIMULATOR_X86_64,
    watch_platforms.WATCHOS_ARM64,
    watch_platforms.WATCHOS_ARM64_32,
    watch_platforms.WATCHSIMULATOR_ARM64,
    watch_platforms.WATCHSIMULATOR_X86_64,
]

_MAC_PLATFORMS = [
    mac_platforms.MACOS_ARM64,
    mac_platforms.MACOS_X86_64,
    mac_platforms.MACOS_UNIVERSAL,
    mac_catalyst_platforms.MACCATALYST_ARM64,
    mac_catalyst_platforms.MACCATALYST_X86_64,
]

# TODO: Drop the platform_rule when we're not longer attempting to support buck1.
def apple_generated_platforms(
    name, constraint_values, deps, platform_rule, platform = None, supported_build_modes = APPLE_BUILD_MODES, use_whatsapp_build_modes = False
):
    # By convention, the cxx.default_platform is typically the same as the platform being defined.
    # This is not the case for all watch platforms, so provide an override.
    platform = platform if platform else name
    build_mode_constraints_map = get_build_mode_constraints_map(use_whatsapp_build_modes)
    if is_mobile_platform(platform) or is_buck2_mac_platform(platform):
        for build_mode in supported_build_modes:
            platform_rule(
                name = _get_generated_name(name, platform, build_mode),
                constraint_values = constraint_values + build_mode_constraints_map[build_mode],
                visibility = ["PUBLIC"],
                deps = deps,
            )

    # Create a platform without the build mode to support backwards compatibility of hardcoded platforms
    # and with buck1 cxx platform setup.
    # TODO(chatatap): Look to remove all hardcoded references and get rid of these
    platform_rule(
        name = name,
        constraint_values = constraint_values,
        visibility = ["PUBLIC"],
        deps = deps,
    )

def apple_build_mode_backed_platform(name, platform, build_mode = None):
    build_mode = get_build_mode() if build_mode == None else build_mode
    return _get_generated_name(name, platform, build_mode)

def is_mobile_platform(platform):
    # These builds modes are primarily used in mobile code. MacOS builds in fbcode/arvr use different
    # modes to represent dev/opt variants.
    return platform in _MOBILE_PLATFORMS

def is_buck2_mac_platform(platform):
    return platform in _MAC_PLATFORMS

def _get_generated_name(name, platform, build_mode):
    if is_mobile_platform(platform) or is_buck2_mac_platform(platform):
        return "{}-{}".format(name, build_mode)
    else:
        return name
