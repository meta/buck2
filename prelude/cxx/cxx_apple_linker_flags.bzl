# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

_PLATFORM_TARGET_TRIPLE_MAP = {
    "appletvos": "{architecture}-apple-tvos{version}",
    "appletvsimulator": "{architecture}-apple-tvos{version}-simulator",
    "iphoneos": "{architecture}-apple-ios{version}",
    "iphonesimulator": "{architecture}-apple-ios{version}-simulator",
    "maccatalyst": "{architecture}-apple-ios{version}-macabi",
    "macosx": "{architecture}-apple-macosx{version}",
    "visionos": "{architecture}-apple-xros{version}",
    "visionsimulator": "{architecture}-apple-xros{version}-simulator",
    "watchos": "{architecture}-apple-watchos{version}",
    "watchsimulator": "{architecture}-apple-watchos{version}-simulator",
}

# - Metal arch is always `air64`, regardless of target arch
# - watchOS does not support targeting Metal
_METAL_PLATFORM_TARGET_TRIPLE_MAP = {
    "appletvos": "air64-apple-tvos{version}",
    "appletvsimulator": "air64-apple-tvos{version}-simulator",
    "iphoneos": "air64-apple-ios{version}",
    "iphonesimulator": "air64-apple-ios{version}-simulator",
    "maccatalyst": "air64-apple-ios{version}-macabi",
    "macosx": "air64-apple-macosx{version}",
    "visionos": "air64-apple-xros{version}",
    "visionsimulator": "air64-apple-xros{version}-simulator",
}

def is_valid_apple_platform_name(platform_name: str | None) -> bool:
    platform_components = (platform_name or "").split("-")
    return platform_components[0] in _PLATFORM_TARGET_TRIPLE_MAP and len(platform_components) == 2

def apple_format_target_triple(platform_name: str, version: str) -> str:
    platform_components = platform_name.split("-")
    if platform_components[0] not in _PLATFORM_TARGET_TRIPLE_MAP:
        fail("missing target triple for {} ({})".format(platform_components[0], platform_name))

    triple_format_str = _PLATFORM_TARGET_TRIPLE_MAP[platform_components[0]]
    return triple_format_str.format(architecture = platform_components[1], version = version)

def apple_format_metal_target_triple(platform: str, version: str) -> str:
    platform_name_and_arch_components = platform.split("-")
    platform_name = platform_name_and_arch_components[0]
    if platform_name not in _METAL_PLATFORM_TARGET_TRIPLE_MAP:
        fail("Missing Metal target triple for {} ({})".format(platform_name, platform))

    metal_triple_format_str = _METAL_PLATFORM_TARGET_TRIPLE_MAP[platform_name]
    return metal_triple_format_str.format(version = version)

def apple_target_triple_flags(target_triple: str | None) -> list[str]:
    return (
        [
            "-target",
            target_triple,
        ]
        if target_triple
        else []
    )

def apple_extra_darwin_linker_flags(target_triple: str | None) -> list[str]:
    """
    Returns a list of linker flags that should be used for all links with a Darwin toolchain.
    """

    # Darwin requires a target triple specified to
    # control the deployment target being linked for.
    extra_linker_flags = apple_target_triple_flags(target_triple)

    # On Apple platforms, DWARF data is contained in the object files
    # and executables contains paths to the object files (N_OSO stab).
    #
    # By default, ld64 will use absolute file paths in N_OSO entries
    # which machine-dependent executables. Such executables would not
    # be debuggable on any host apart from the host which performed
    # the linking. Instead, we want produce machine-independent
    # hermetic executables, so we need to relativize those paths.
    #
    # This is accomplished by passing the `oso-prefix` flag to ld64,
    # which will strip the provided prefix from the N_OSO paths.
    #
    # The flag accepts a special value, `.`, which means it will
    # use the current workding directory. This will make all paths
    # relative to the parent of `buck-out`.
    #
    # Because all actions in Buck2 are run from the project root
    # and `buck-out` is always inside the project root, we can
    # safely pass `.` as the `-oso_prefix` without having to
    # write a wrapper script to compute it dynamically.
    extra_linker_flags.append("-Wl,-oso_prefix,.")
    return extra_linker_flags
