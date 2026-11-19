# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:paths.bzl", "paths")
load("@prelude//apple:apple_toolchain_types.bzl", "AppleToolchainInfo")
load(":apple_bundle_destination.bzl", "AppleBundleDestination")
load(":apple_bundle_part.bzl", "AppleBundlePart", "assemble_bundle")
load(":apple_bundle_types.bzl", "AppleBundleInfo", "AppleBundleType")
load(":apple_info_plist.bzl", "process_info_plist")
load(":apple_utility.bzl", "get_apple_architecture")

def apple_xcuitest_impl(ctx: AnalysisContext) -> [list[Provider], Promise]:
    # The XCUITest runner app bundle copies the application from the platform
    # directory, and includes the UI test bundle in the PlugIns folder.
    output_bundle = ctx.actions.declare_output(ctx.attrs.name + "." + ctx.attrs.extension, has_content_based_path = False)
    bundle_parts = [
        _get_xctrunner_binary(ctx),
        _get_uitest_bundle(ctx),
    ] + _get_xctrunner_frameworks(ctx)
    bundle_result = assemble_bundle(
        ctx = ctx,
        bundle = output_bundle,
        info_plist_part = process_info_plist(ctx, override_input = None),
        parts = bundle_parts,
        codesign_manifest_parts = [],
        signing_context_parts = [],
        swift_stdlib_args = None,
    )

    return [
        DefaultInfo(default_output = output_bundle),
        AppleBundleInfo(
            bundle = output_bundle,
            bundle_type = AppleBundleType("default"),
            binary_name = ctx.attrs.name,
            contains_watchapp = False,
            # The test runner binary does not contain Swift
            skip_copying_swift_stdlib = True,
        ),
    ] + bundle_result.providers

def _get_uitest_bundle(ctx: AnalysisContext) -> AppleBundlePart:
    return AppleBundlePart(
        source = ctx.attrs.test_bundle[DefaultInfo].default_outputs[0],
        destination = AppleBundleDestination("plugins"),
    )

def _get_xctrunner_binary(ctx: AnalysisContext) -> AppleBundlePart:
    arch = get_apple_architecture(ctx)
    lipo = ctx.attrs._apple_toolchain[AppleToolchainInfo].lipo
    platform_path = ctx.attrs._apple_toolchain[AppleToolchainInfo].platform_path
    thin_binary = ctx.actions.declare_output(ctx.attrs.name, has_content_based_path = False)
    xctrunner_path = cmd_args(platform_path, "Developer/Library/Xcode/Agents/XCTRunner.app/XCTRunner", delimiter = "/")
    ctx.actions.run(
        [
            lipo,
            xctrunner_path,
            "-extract",
            arch,
            "-output",
            thin_binary.as_output(),
        ],
        category = "copy_xctrunner",
    )

    return AppleBundlePart(
        source = thin_binary,
        destination = AppleBundleDestination("executables"),
    )

def _get_xctrunner_frameworks(ctx: AnalysisContext) -> list[AppleBundlePart]:
    # We need to copy the framework as AppleBundlePart requires an artifact.
    # It would be nicer to make this an arglike and avoid the copies.
    # It would also be nicer to exclude the headers.
    def copy_platform_framework(platform_relative_path: str) -> AppleBundlePart:
        copied_framework = ctx.actions.declare_output(paths.basename(platform_relative_path), has_content_based_path = False)
        path = cmd_args(ctx.attrs._apple_toolchain[AppleToolchainInfo].platform_path, platform_relative_path, delimiter = "/")
        ctx.actions.run(["cp", "-PR", path, copied_framework.as_output()], category = "copy_framework", identifier = platform_relative_path)
        return AppleBundlePart(
            source = copied_framework,
            destination = AppleBundleDestination("frameworks"),
            codesign_on_copy = True,
        )

    runner_frameworks = [
        "Developer/Library/Frameworks/XCTest.framework",
        "Developer/Library/PrivateFrameworks/XCTAutomationSupport.framework",
        "Developer/Library/PrivateFrameworks/XCTestCore.framework",
        "Developer/Library/PrivateFrameworks/XCTestSupport.framework",
        "Developer/Library/PrivateFrameworks/XCUnit.framework",
    ]

    # From Xcode 16.3 XCUIAutomation is in Frameworks, not PrivateFrameworks
    xcode_version = int(ctx.attrs._apple_toolchain[AppleToolchainInfo].xcode_version)
    if xcode_version >= 1630:
        runner_frameworks.append("Developer/Library/Frameworks/XCUIAutomation.framework")
    else:
        runner_frameworks.append("Developer/Library/PrivateFrameworks/XCUIAutomation.framework")
    return [copy_platform_framework(p) for p in runner_frameworks]
