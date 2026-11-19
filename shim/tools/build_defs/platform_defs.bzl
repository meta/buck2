# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    "@fbsource//tools/build_defs/default_platform_defs.bzl",
    _ANDROID = "ANDROID",
    _APPLE = "APPLE",
    _APPLETVOS = "APPLETVOS",
    _CXX = "CXX",
    _FBCODE = "FBCODE",
    _IOS = "IOS",
    _MACOSX = "MACOSX",
    _WATCHOS = "WATCHOS",
    _WINDOWS = "WINDOWS",
)

ANDROID = _ANDROID
APPLE = _APPLE
APPLETVOS = _APPLETVOS
CXX = _CXX
FBCODE = _FBCODE
IOS = _IOS
MACOSX = _MACOSX
WATCHOS = _WATCHOS
WINDOWS = _WINDOWS
ALL_APPLE_SDKS = (_IOS, _APPLETVOS, _MACOSX, _WATCHOS)

def xplat_target_compatible_with(platforms, apple_sdks = None):
    if platforms == None:
        return []
    if type(platforms) == "string":
        platforms = (platforms,)

    incompatible = ["ovr_config//:none"]
    compatible = {
        "DEFAULT": [] if CXX in platforms else incompatible,
        "ovr_config//os:android": [] if ANDROID in platforms else incompatible,
        "ovr_config//os:linux": [] if CXX in platforms else incompatible,
        "ovr_config//os:windows": [] if WINDOWS in platforms else incompatible,
    }
    if APPLE in platforms:
        if apple_sdks == None:
            apple_sdks = (IOS, APPLETVOS, MACOSX)
        if type(apple_sdks) == "string":
            apple_sdks = (apple_sdks,)
    else:
        apple_sdks = ()
    for sdk, config in (
        (IOS, "ovr_config//os:iphoneos"),
        (APPLETVOS, "ovr_config//os:appletvos"),
        (MACOSX, "ovr_config//os:macos"),
        (WATCHOS, "ovr_config//os:watchos"),
    ):
        compatible[config] = [] if sdk in apple_sdks else incompatible
    return select(compatible)
