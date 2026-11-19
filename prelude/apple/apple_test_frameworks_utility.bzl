# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:paths.bzl", "paths")
load("@prelude//apple:apple_bundle_destination.bzl", "AppleBundleDestination")
load("@prelude//apple:apple_bundle_part.bzl", "AppleBundlePart")
load("@prelude//apple:apple_toolchain_types.bzl", "AppleToolchainInfo")

def get_test_frameworks_bundle_parts(ctx: AnalysisContext, swift_support_needed: bool) -> list[AppleBundlePart]:
    xcode_version = int(ctx.attrs._apple_toolchain[AppleToolchainInfo].xcode_version)
    paths = [
        _get_object_from_platform_path(ctx, "Developer/Library/Frameworks/XCTest.framework"),
        _get_object_from_platform_path(ctx, "Developer/Library/PrivateFrameworks/XCTAutomationSupport.framework"),
        _get_object_from_platform_path(ctx, "Developer/Library/PrivateFrameworks/XCTestCore.framework"),
        _get_object_from_platform_path(ctx, "Developer/Library/PrivateFrameworks/XCTestSupport.framework"),
        _get_object_from_platform_path(ctx, "Developer/Library/PrivateFrameworks/XCUnit.framework"),
        _get_object_from_platform_path(ctx, "Developer/usr/lib/libXCTestBundleInject.dylib"),
    ]

    # From Xcode 16.3 XCUIAutomation is in Frameworks, not PrivateFrameworks
    if xcode_version >= 1630:
        paths.append(_get_object_from_platform_path(ctx, "Developer/Library/Frameworks/XCUIAutomation.framework"))
    else:
        paths.append(_get_object_from_platform_path(ctx, "Developer/Library/PrivateFrameworks/XCUIAutomation.framework"))

    if swift_support_needed or ctx.attrs.swift_testing:
        paths.append(_get_object_from_platform_path(ctx, "Developer/usr/lib/libXCTestSwiftSupport.dylib"))

        # T201426509: Xcode 16 introduces the Swift Testing framework
        # that is a load dependency of libXCTestSwiftSupport.dylib
        if xcode_version >= 1600:
            paths.append(_get_object_from_platform_path(ctx, "Developer/Library/Frameworks/Testing.framework"))
        if xcode_version >= 2600:
            paths.append(_get_object_from_platform_path(ctx, "Developer/Library/Frameworks/_Testing_Foundation.framework"))

        # Xcode 26.4 makes Testing.framework load lib_TestingInterop.dylib at runtime.
        if xcode_version >= 2640:
            paths.append(_get_object_from_platform_path(ctx, "Developer/usr/lib/lib_TestingInterop.dylib"))

    return paths

def _get_object_from_platform_path(ctx: AnalysisContext, platform_relative_path: str) -> AppleBundlePart:
    toolchain = ctx.attrs._apple_toolchain[AppleToolchainInfo]
    copied_framework = ctx.actions.declare_output(paths.basename(platform_relative_path), has_content_based_path = False)

    # We have to copy because:
    # 1) Platform path might be a string (e.g. for Xcode toolchains)
    # 2) It's not possible to project artifact which is not produced by different target (and platform path is a separate target for distributed toolchains).
    ctx.actions.run(
        ["cp", "-PR", cmd_args(toolchain.platform_path, platform_relative_path, delimiter = "/"), copied_framework.as_output()],
        category = "extract_framework",
        identifier = platform_relative_path,
    )

    return AppleBundlePart(source = copied_framework, destination = AppleBundleDestination("frameworks"), codesign_on_copy = True)
