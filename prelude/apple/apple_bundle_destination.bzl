# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:paths.bzl", "paths")
load(":apple_bundle_utility.bzl", "get_apple_versioned_macos_bundle_value_primitive")

# Abstraction of a place in a resulting bundle where file or directory will be copied. Actual value
# of path relative to bundle root depends on a platform. This class is an implementation detail and
# is not exposed to user unlike `AppleResourceDestination`.
# v1 code is `com/facebook/buck/apple/AppleBundleDestination.java`
AppleBundleDestination = enum(
    "resources",
    "frameworks",
    "executables",
    "plugins",
    "xpcservices",
    "metadata",
    "watchapp",
    "headers",
    "modules",
    "quicklook",
    "bundleroot",
    "loginitems",
    "launchagents",
    "launchdaemons",
    "appclips",
    "extensionkit_extensions",
)

AppleBundleDestinationPaths = record(
    resources = field(str, ""),
    frameworks = field(str, ""),
    executables = field(str, ""),
    plugins = field(str, ""),
    xpcservices = field(str, ""),
    metadata = field(str, ""),
    watchapp = field(str, ""),
    headers = field(str, ""),
    modules = field(str, ""),
    quicklook = field(str, ""),
    bundleroot = field(str, ""),
    loginitems = field(str, ""),
    launchagents = field(str, ""),
    launchdaemons = field(str, ""),
    appclips = field(str, ""),
    extensionkit_extensions = field(str, ""),
)

_IOSBundleDestinationPaths = AppleBundleDestinationPaths(
    frameworks = "Frameworks",
    plugins = "PlugIns",
    xpcservices = "XPCServices",
    watchapp = "Watch",
    quicklook = "Library/QuickLook",
    appclips = "AppClips",
    extensionkit_extensions = "Extensions",
)

_IOSFrameworkBundleDestinationPaths = AppleBundleDestinationPaths(
    frameworks = "Frameworks",
    xpcservices = "XPCServices",
    headers = "Headers",
    modules = "Modules",
)

macOS_content_path = "Contents"
_MacOSBundleDestinationPaths = AppleBundleDestinationPaths(
    resources = paths.join(macOS_content_path, "Resources"),
    frameworks = paths.join(macOS_content_path, "Frameworks"),
    executables = paths.join(macOS_content_path, "MacOS"),
    plugins = paths.join(macOS_content_path, "PlugIns"),
    xpcservices = paths.join(macOS_content_path, "XPCServices"),
    metadata = macOS_content_path,
    watchapp = macOS_content_path,
    headers = macOS_content_path,
    modules = macOS_content_path,
    quicklook = paths.join(macOS_content_path, "Library/QuickLook"),
    bundleroot = macOS_content_path,
    loginitems = paths.join(macOS_content_path, "Library/LoginItems"),
    launchagents = paths.join(macOS_content_path, "Library/LaunchAgents"),
    launchdaemons = paths.join(macOS_content_path, "Library/LaunchDaemons"),
    extensionkit_extensions = paths.join(macOS_content_path, "Extensions"),
)

_MacOSFrameworkBundleDestinationPaths = AppleBundleDestinationPaths(
    resources = "Resources",
    frameworks = "Frameworks",
    plugins = "PlugIns",
    xpcservices = "XPCServices",
    metadata = "Resources",
    headers = "Headers",
    modules = "Modules",
)

macOS_versioned_path = "Versions/A"
_MacOSVersionedFrameworkBundleDestinationPaths = AppleBundleDestinationPaths(
    resources = paths.join(macOS_versioned_path, "Resources"),
    frameworks = paths.join(macOS_versioned_path, "Frameworks"),
    plugins = paths.join(macOS_versioned_path, "PlugIns"),
    xpcservices = paths.join(macOS_versioned_path, "XPCServices"),
    metadata = paths.join(macOS_versioned_path, "Resources"),
    headers = paths.join(macOS_versioned_path, "Headers"),
    modules = paths.join(macOS_versioned_path, "Modules"),
    executables = macOS_versioned_path,
)

def _get_apple_bundle_destinations_for_sdk_name(name: str) -> AppleBundleDestinationPaths:
    if name == "macosx" or name == "maccatalyst":
        return _MacOSBundleDestinationPaths
    else:
        return _IOSBundleDestinationPaths

def _get_apple_framework_bundle_destinations_for_sdk_name(name: str, versioned_macos_bundle: bool) -> AppleBundleDestinationPaths:
    if name.startswith("mac"):
        if get_apple_versioned_macos_bundle_value_primitive(name, versioned_macos_bundle):
            return _MacOSVersionedFrameworkBundleDestinationPaths
        else:
            return _MacOSFrameworkBundleDestinationPaths
    else:
        return _IOSFrameworkBundleDestinationPaths

def bundle_relative_path_for_destination(destination: AppleBundleDestination, sdk_name: str, extension: str, versioned_macos_bundle: bool) -> str:
    if extension == "framework":
        bundle_destinations = _get_apple_framework_bundle_destinations_for_sdk_name(sdk_name, versioned_macos_bundle)
    else:
        bundle_destinations = _get_apple_bundle_destinations_for_sdk_name(sdk_name)

    if destination == AppleBundleDestination("resources"):
        return bundle_destinations.resources
    elif destination == AppleBundleDestination("frameworks"):
        return bundle_destinations.frameworks
    elif destination == AppleBundleDestination("executables"):
        return bundle_destinations.executables
    elif destination == AppleBundleDestination("extensionkit_extensions"):
        return bundle_destinations.extensionkit_extensions
    elif destination == AppleBundleDestination("plugins"):
        return bundle_destinations.plugins
    elif destination == AppleBundleDestination("xpcservices"):
        return bundle_destinations.xpcservices
    elif destination == AppleBundleDestination("metadata"):
        return bundle_destinations.metadata
    elif destination == AppleBundleDestination("watchapp"):
        return bundle_destinations.watchapp
    elif destination == AppleBundleDestination("headers"):
        return bundle_destinations.headers
    elif destination == AppleBundleDestination("modules"):
        return bundle_destinations.modules
    elif destination == AppleBundleDestination("quicklook"):
        return bundle_destinations.quicklook
    elif destination == AppleBundleDestination("bundleroot"):
        return bundle_destinations.bundleroot
    elif destination == AppleBundleDestination("loginitems"):
        return bundle_destinations.loginitems
    elif destination == AppleBundleDestination("launchagents"):
        return bundle_destinations.launchagents
    elif destination == AppleBundleDestination("launchdaemons"):
        return bundle_destinations.launchdaemons
    elif destination == AppleBundleDestination("appclips"):
        return bundle_destinations.appclips
    fail("Unsupported Apple bundle destination {}".format(destination))
