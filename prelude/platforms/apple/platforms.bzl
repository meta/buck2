# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//platforms/apple:base.bzl", "apple_build_mode_backed_platform", "get_build_mode_constraints_map", "is_buck2_mac_platform", "is_mobile_platform")
load(
    "@prelude//platforms/apple:build_mode.bzl",
    "APPLE_BUILD_MODES",
    "REMAPPED_BUILD_MODES",
    "get_build_mode",
    "get_build_mode_debug",
)
load(
    "@prelude//platforms/apple:constants.bzl",
    "IOS",
    "appletv_platforms",
    "ios_platforms",
    "mac_catalyst_platforms",
    "mac_platforms",
    "watch_platforms",
)
load("@prelude//platforms/apple:platforms_map.bzl", "APPLE_PLATFORMS_MAP", "APPLE_SDK_DEFAULT_PLATFORM_MAP")
load("@prelude//utils:buckconfig.bzl", "read")

_SUPPORTED_IOS_PLATFORMS = [
    ios_platforms.IPHONEOS_ARM64,
    ios_platforms.IPHONESIMULATOR_ARM64,
]

_SUPPORTED_APPLETV_PLATFORMS = [
    appletv_platforms.APPLETVOS_ARM64,
    appletv_platforms.APPLETVSIMULATOR_ARM64,
]

_SUPPORTED_MACOS_PLATFORMS = [
    mac_platforms.MACOS_ARM64,
    mac_platforms.MACOS_X86_64,
]

_SUPPORTED_MAC_CATALYST_PLATFORMS = [
    mac_catalyst_platforms.MACCATALYST_ARM64,
    mac_catalyst_platforms.MACCATALYST_X86_64,
]

_SUPPORTED_WATCHOS_PLATFORMS = [
    watch_platforms.WATCHOS_ARM64,
    watch_platforms.WATCHOS_ARM64_32,
    watch_platforms.WATCHSIMULATOR_ARM64,
]

_ANALYSIS_CONSTRAINTS = ["ovr_config//bitcode/constraints:bitcode_mode[bitcode]"]
_DEFAULT_ANALYSIS_IOS_PLATFORM = ios_platforms.IPHONEOS_ARM64
_DEFAULT_ANALYSIS_MACOS_PLATFORM = mac_platforms.MACOS_ARM64
_DEFAULT_ANALYSIS_WATCHOS_PLATFORM = watch_platforms.WATCHOS_ARM64
_DEFAULT_ANALYSIS_APPLETVOS_PLATFORM = appletv_platforms.APPLETVOS_ARM64

DEFAULT_SUPPORTED_CXX_PLATFORMS = _SUPPORTED_IOS_PLATFORMS

INVERSE_REMAPPED_BUILD_MODES = {v: k for k, v in REMAPPED_BUILD_MODES.items()}

def apple_target_platforms(
    base_name: str,
    platform_rule,
    constraint_values = None,  # Constraint values added to all generated platforms
    visibility = None,
    deps = None,
    cxx_platforms_constraint_values = None,  # Must be a map of a supported cxx platform to a list of constraint values
    build_mode_constraint_values = None,  # Must be a map of a supported build mode to a list of constraint values
    generate_base_platform = True,  # Whether to generate a base platform
    add_config_based_platforms = True,  # Whether to add the configured cxx platform family when it is otherwise unsupported
    use_whatsapp_build_modes = False,
    supported_cxx_platforms = DEFAULT_SUPPORTED_CXX_PLATFORMS,  # Cxx platforms to generate platforms for
    supported_build_modes = APPLE_BUILD_MODES,
) -> None:  # Build modes to generate platforms for
    """Define architecture and sdk specific platforms alongside the base platform."""

    # HACK: Apps shouldn't be generating platforms for cxx_platforms they don't support. However, to support cases where other apps
    # depend on shared libraries that don't generate particular platforms, and set a cxx.default_platform on the command line, we need
    # to make the graph parseable and generate the missing target platforms. They will never be used, but need to exist in the config
    # backed world.
    config_based_platform = read("cxx", "default_platform")
    if add_config_based_platforms and config_based_platform != None and config_based_platform not in supported_cxx_platforms:
        supported_cxx_platforms = list(supported_cxx_platforms)
        for platform_family in [
            _SUPPORTED_MACOS_PLATFORMS,
            _SUPPORTED_MAC_CATALYST_PLATFORMS,
            _SUPPORTED_IOS_PLATFORMS,
            _SUPPORTED_WATCHOS_PLATFORMS,
            _SUPPORTED_APPLETV_PLATFORMS,
        ]:
            if config_based_platform in platform_family:
                for p in platform_family:
                    if p not in supported_cxx_platforms:
                        supported_cxx_platforms.append(p)

    # Form defaults
    constraint_values = constraint_values or []
    cxx_platforms_constraint_values = cxx_platforms_constraint_values or {}
    build_mode_constraint_values = build_mode_constraint_values or {}
    visibility = visibility or ["PUBLIC"]
    deps = deps or []

    _validate_cxx_platforms_constraint_values(base_name, cxx_platforms_constraint_values, supported_cxx_platforms)
    _validate_build_mode_constraint_values(base_name, build_mode_constraint_values, supported_build_modes)

    # Define the generated platforms
    build_mode_constraints_map = get_build_mode_constraints_map(use_whatsapp_build_modes)
    for platform in supported_cxx_platforms:
        platform_dep = _get_base_target_platform_for_platform(platform)
        cxx_platform_constraints = cxx_platforms_constraint_values.get(platform, [])
        if is_mobile_platform(platform) or is_buck2_mac_platform(platform):
            for build_mode in supported_build_modes:
                build_mode_constraints = build_mode_constraint_values.get(build_mode, []) + build_mode_constraints_map[build_mode]
                _define_platform(
                    base_name,
                    platform,
                    build_mode,
                    constraint_values + cxx_platform_constraints + build_mode_constraints,
                    visibility,
                    deps + [platform_dep],
                    platform_rule,
                )
        else:
            _define_platform(
                base_name,
                platform,
                None,
                constraint_values + cxx_platform_constraints,
                visibility,
                deps + [platform_dep],
                platform_rule,
            )

    # Define the base platform in case it is needed (example: to be a dep of another platform)
    if generate_base_platform:
        platform_rule(
            name = base_name,
            constraint_values = constraint_values,
            visibility = visibility,
            deps = deps,
        )

        analysis_platform = _get_analysis_platform_for_supported_platforms(supported_cxx_platforms)
        analysis_platform_dep = _get_base_target_platform_for_platform(analysis_platform)
        analysis_platform_build_mode_constraints = build_mode_constraint_values.get(get_build_mode_debug(), [])

        platform_rule(
            name = base_name + "-analysis",
            constraint_values = constraint_values + analysis_platform_build_mode_constraints + _ANALYSIS_CONSTRAINTS,
            visibility = ["PUBLIC"],
            deps = deps + [analysis_platform_dep],
        )

def config_backed_apple_target_platform(target_platform = None, platform = None, build_mode = None, supported_build_modes = None) -> str | None:
    platform = _get_default_platform() if platform == None else platform

    build_mode = get_build_mode() if build_mode == None else build_mode
    supported_build_modes = APPLE_BUILD_MODES if supported_build_modes == None else supported_build_modes
    if build_mode not in supported_build_modes:
        # If build_mode is an unsupported build mode, attempt to map it to a supported one using the inverse map
        build_mode = INVERSE_REMAPPED_BUILD_MODES.get(build_mode, build_mode)

    if target_platform == None:
        return get_default_target_platform_for_platform(platform)

    return _get_generated_name(target_platform, platform, build_mode)

def get_default_target_platform_for_platform(sdk_arch) -> str | None:
    data = APPLE_PLATFORMS_MAP.get(sdk_arch)
    if data != None:
        return data.target_platform

    return None

def _get_generated_name(base_name, platform, build_mode) -> str:
    platform_and_build_mode_name = apple_build_mode_backed_platform(platform, platform, build_mode)
    return "{}-{}".format(base_name, platform_and_build_mode_name)

def _get_default_platform() -> str:
    platform = read("cxx", "default_platform")
    return platform if platform != None else APPLE_SDK_DEFAULT_PLATFORM_MAP.get(IOS)

def _define_platform(base_name, platform, build_mode, constraint_values, visibility, deps, platform_rule) -> None:
    platform_rule(
        name = _get_generated_name(base_name, platform, build_mode),
        constraint_values = constraint_values,
        visibility = visibility,
        deps = deps,
    )

def _get_base_target_platform_for_platform(sdk_arch) -> str | None:
    data = APPLE_PLATFORMS_MAP.get(sdk_arch)
    if data != None:
        return data.base_target_platform

    return None

def _get_analysis_platform_for_supported_platforms(supported_cxx_platforms) -> str:
    # For determining the platform deps to use for the base platform, we inspect the supported
    # cxx platforms, giving precedence to iOS platforms.
    for platform in _SUPPORTED_IOS_PLATFORMS:
        if platform in supported_cxx_platforms:
            return _DEFAULT_ANALYSIS_IOS_PLATFORM

    for platform in _SUPPORTED_MACOS_PLATFORMS:
        if platform in supported_cxx_platforms:
            return _DEFAULT_ANALYSIS_MACOS_PLATFORM

    for platform in _SUPPORTED_WATCHOS_PLATFORMS:
        if platform in supported_cxx_platforms:
            return _DEFAULT_ANALYSIS_WATCHOS_PLATFORM

    for platform in _SUPPORTED_APPLETV_PLATFORMS:
        if platform in supported_cxx_platforms:
            return _DEFAULT_ANALYSIS_APPLETVOS_PLATFORM

    return _DEFAULT_ANALYSIS_IOS_PLATFORM

def _validate_cxx_platforms_constraint_values(base_name, cxx_platforms_constraint_values, supported_cxx_platforms) -> None:
    if type(cxx_platforms_constraint_values) != type({}):
        fail("cxx_platforms_constraint_values must be a map of platform to constraint values!")
    for platform, platform_values in cxx_platforms_constraint_values.items():
        if platform not in supported_cxx_platforms:
            fail(
                "\n\nProviding platform constraints for an unsupported platform!\nBase platform: {}\nCXX Platform: {} with values {}\nSupported platforms: {}\n".format(
                    base_name,
                    platform,
                    platform_values,
                    ", ".join(supported_cxx_platforms),
                )
            )

def _validate_build_mode_constraint_values(base_name, build_mode_constraint_values, supported_build_modes) -> None:
    if type(build_mode_constraint_values) != type({}):
        fail("build_mode_constraint_values must be a map of build mode to constraint values!")
    for build_mode, build_mode_values in build_mode_constraint_values.items():
        if build_mode not in supported_build_modes:
            fail(
                "\n\nProviding build mode constraints for an unsupported build mode!\nBase platform: {}\nBuild mode: {} with values {}\nSupported build modes: {}\n".format(
                    base_name,
                    build_mode,
                    build_mode_values,
                    ", ".join(supported_build_modes),
                )
            )
