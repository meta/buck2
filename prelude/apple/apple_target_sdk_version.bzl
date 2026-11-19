# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//apple:versions.bzl", "TARGET_SDK_VERSIONS")
load("@prelude//cxx:target_sdk_version.bzl", "get_target_sdk_version")

# Taken from SDKSettings.json[VersionMap][iOSMac_macOS]
_MACCATALYST_IOS_TO_MACOS_VERSION_MAP = {
    "13.0": "10.15",  # Catalina
    "13.1": "10.15",
    "13.2": "10.15.1",
    "13.3": "10.15.2",
    "13.3.1": "10.15.3",
    "13.4": "10.15.4",
    "13.5": "10.15.5",
    "13.6": "10.15.5",  # Xcode reported 10.15
    "14.0": "11.0",  # Big Sur
    "14.1": "11.0",
    "14.2": "11.0",
    "14.3": "11.1",
    "14.4": "11.2",
    "14.5": "11.3",
    "14.6": "11.4",
    "14.7": "11.5",
    "15.0": "12.0",  # Monterey
    "15.1": "12.1",  # Xcode reported 10.15
    "15.2": "12.1",
    "15.3": "12.2",
    "15.4": "12.3",
    "15.5": "12.4",
    "15.6": "12.5",
    "16.0": "13.0",  # Ventura
    "16.1": "13.0",
    "16.2": "13.1",
    "16.3": "13.2",
    "16.4": "13.3",
    "16.5": "13.4",
    "16.6": "13.5",
    "17.0": "14.0",  # Sonoma
    "17.1": "14.1",
    "17.2": "14.2",
    "17.3": "14.3",
    "17.4": "14.4",
    "17.5": "14.5",
    "18.0": "15.0",  # Sequoia
    "18.1": "15.1",
    "18.2": "15.2",
    "18.3": "15.3",
    "18.4": "15.4",
    "18.5": "15.5",
    "18.6": "15.6",
    "19.0": "16.0",  # Tahoe
}

_SDK_NAME_TO_PLATFORM_NAME_OVERRIDE_MAP = {
    "maccatalyst": "macosx",
}

def get_target_sdk_version_map() -> dict[str, str]:
    return {
        maccatalyst_version: "config//version:constraint-value-target-sdk-version-{}".format(macos_version)
        for maccatalyst_version, macos_version in _MACCATALYST_IOS_TO_MACOS_VERSION_MAP.items()
        if macos_version in TARGET_SDK_VERSIONS
    }

def get_platform_version_for_sdk_version(sdk_name: str, sdk_version: str) -> str:
    if sdk_name == "maccatalyst":
        sdk_major = sdk_version.split(".")[0]

        # Version numbers are aligned from 26 and up
        if int(sdk_major) >= 26:
            return sdk_version

        macos_version = _MACCATALYST_IOS_TO_MACOS_VERSION_MAP.get(sdk_version, None)
        if macos_version == None:
            fail("No macos version for maccatalyst version {}".format(sdk_version))
        return macos_version

    return sdk_version

def get_platform_name_for_sdk(sdk_name: str) -> str:
    return _SDK_NAME_TO_PLATFORM_NAME_OVERRIDE_MAP.get(sdk_name, sdk_name)

# Returns the target_sdk_version specified for this build, falling
# back to the toolchain version when unset.
def get_min_deployment_version_for_node(ctx: AnalysisContext) -> str:
    version = get_target_sdk_version(ctx)
    if version == None:
        fail("No target_sdk_version set on target or toolchain")

    return version
