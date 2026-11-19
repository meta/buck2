# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# These are identifiers used in defining Apple platforms for configuring apple_* rules.

APPLE = "Apple"

# Apple SDK Definitions
APPLETVOS = "appletvos"

IOS = "ios"

MACOSX = "macosx"

WATCHOS = "watchos"

VISIONOS = "visionos"

# Apple TV Platforms/Flavors

APPLETVOS_ARM64 = "appletvos-arm64"

APPLETVSIMULATOR_ARM64 = "appletvsimulator-arm64"

# iOS Platforms/Flavors

IPHONEOS_ARM64 = "iphoneos-arm64"

IPHONEOS_ARM64E = "iphoneos-arm64e"

IPHONESIMULATOR_ARM64 = "iphonesimulator-arm64"

IPHONESIMULATOR_X86_64 = "iphonesimulator-x86_64"

# Mac Catalyst Platforms/Flavors

MACCATALYST_ARM64 = "maccatalyst-arm64"

MACCATALYST_X86_64 = "maccatalyst-x86_64"

# Mac OS X Platforms/Flavors

MACOS_ARM64 = "macosx-arm64"

MACOS_X86_64 = "macosx-x86_64"

MACOS_UNIVERSAL = "macosx-universal"

# Watch OS Platforms/Flavors

WATCHOS_ARM64 = "watchos-arm64"

WATCHOS_ARM64_32 = "watchos-arm64_32"

WATCHSIMULATOR_ARM64 = "watchsimulator-arm64"

WATCHSIMULATOR_X86_64 = "watchsimulator-x86_64"

# Vision OS Platforms/Flavors
VISIONOS_ARM64 = "visionos-arm64"

VISIONSIMULATOR_ARM64 = "visionsimulator-arm64"

apple_sdks = struct(
    IOS = IOS,
    WATCHOS = WATCHOS,
    MACOSX = MACOSX,
    APPLETVOS = APPLETVOS,
    VISIONOS = VISIONOS,
)

ALL_APPLE_SDKS = tuple(
    [getattr(apple_sdks, field) for field in dir(apple_sdks)]
)  # convert the `apple_sdks` struct into a tuple (taking the values, not the keys)

appletv_platforms = struct(
    APPLETVOS_ARM64 = APPLETVOS_ARM64,
    APPLETVSIMULATOR_ARM64 = APPLETVSIMULATOR_ARM64,
)

ios_platforms = struct(
    IPHONEOS_ARM64 = IPHONEOS_ARM64,
    IPHONEOS_ARM64E = IPHONEOS_ARM64E,
    IPHONESIMULATOR_ARM64 = IPHONESIMULATOR_ARM64,
    IPHONESIMULATOR_X86_64 = IPHONESIMULATOR_X86_64,
)

mac_catalyst_platforms = struct(
    MACCATALYST_ARM64 = MACCATALYST_ARM64,
    MACCATALYST_X86_64 = MACCATALYST_X86_64,
)

mac_platforms = struct(
    MACOS_ARM64 = MACOS_ARM64,
    MACOS_X86_64 = MACOS_X86_64,
    MACOS_UNIVERSAL = MACOS_UNIVERSAL,
)

watch_platforms = struct(
    WATCHOS_ARM64 = WATCHOS_ARM64,
    WATCHOS_ARM64_32 = WATCHOS_ARM64_32,
    WATCHSIMULATOR_ARM64 = WATCHSIMULATOR_ARM64,
    WATCHSIMULATOR_X86_64 = WATCHSIMULATOR_X86_64,
)

vision_platforms = struct(
    VISIONOS_ARM64 = VISIONOS_ARM64,
    VISIONSIMULATOR_ARM64 = VISIONSIMULATOR_ARM64,
)
