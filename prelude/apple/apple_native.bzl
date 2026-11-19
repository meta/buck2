# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude.bzl", "native")
load(
    "@prelude//platforms/apple:constants.bzl",
    "apple_sdks",
)
load("@prelude//platforms/apple:platforms.bzl", "config_backed_apple_target_platform", "get_default_target_platform_for_platform")
load("@prelude//platforms/apple:platforms_map.bzl", "APPLE_SDK_DEFAULT_PLATFORM_MAP")
load("@prelude//utils:buckconfig.bzl", "read")

def _apple_library(**kwargs):
    kwargs = update_apple_platforms(**kwargs)
    native.apple_library(**kwargs)

def _apple_asset_catalog(**kwargs):
    kwargs = update_apple_platforms(**kwargs)
    native.apple_asset_catalog(**kwargs)

def _apple_binary(**kwargs):
    kwargs = update_apple_platforms(**kwargs)
    native.apple_binary(**kwargs)

def _apple_bundle(**kwargs):
    kwargs = update_apple_platforms(**kwargs)
    native.apple_bundle(**kwargs)

def _apple_watchos_bundle(**kwargs):
    kwargs = update_apple_platforms(**kwargs)
    native.apple_watchos_bundle(**kwargs)

def _apple_package(**kwargs):
    kwargs = update_apple_platforms(**kwargs)
    native.apple_package(**kwargs)

def _apple_resource(**kwargs):
    kwargs = update_apple_platforms(**kwargs)
    native.apple_resource(**kwargs)

def _apple_test(**kwargs):
    kwargs = update_apple_platforms(**kwargs)
    native.apple_test(**kwargs)

def _apple_xcuitest(**kwargs):
    kwargs = update_apple_platforms(**kwargs)
    native.apple_xcuitest(**kwargs)

def _apple_xcframework(**kwargs):
    kwargs = update_apple_platforms(**kwargs)
    native.apple_xcframework(**kwargs)

def update_apple_platforms(**kwargs):
    sdk = kwargs.pop("sdk", apple_sdks.IOS)
    platform = _get_default_platform(sdk)

    default_target_platform = kwargs.pop("default_target_platform", None)
    base_config_backed_target_platform = kwargs.pop("config_backed_target_platform", None)

    if default_target_platform != None and base_config_backed_target_platform != None:
        name = kwargs.get("name", "UNKNOWN_TARGET")
        fail("{} has both a default_target_platform and a config_backed_target_platform, which is not allowed".format(name))

    supported_build_modes = kwargs.pop("supported_build_modes", None)

    if base_config_backed_target_platform != None:
        default_target_platform = config_backed_apple_target_platform(base_config_backed_target_platform, platform, supported_build_modes = supported_build_modes)
    elif default_target_platform == None:
        default_target_platform = get_default_target_platform_for_platform(platform)

    if default_target_platform != None:
        kwargs["default_target_platform"] = default_target_platform

    return kwargs

def _get_default_platform(sdk: str) -> str:
    config_platform = read("cxx", "default_platform")
    if config_platform != None:
        return config_platform
    return APPLE_SDK_DEFAULT_PLATFORM_MAP.get(sdk)

apple_native = struct(
    apple_asset_catalog = _apple_asset_catalog,
    apple_binary = _apple_binary,
    apple_bundle = _apple_bundle,
    apple_watchos_bundle = _apple_watchos_bundle,
    apple_library = _apple_library,
    apple_package = _apple_package,
    apple_resource = _apple_resource,
    apple_test = _apple_test,
    apple_xcuitest = _apple_xcuitest,
    apple_xcframework = _apple_xcframework,
)
