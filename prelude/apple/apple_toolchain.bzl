# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//apple:apple_toolchain_types.bzl", "AppleToolchainInfo")
load("@prelude//apple/swift:swift_toolchain_types.bzl", "SwiftToolchainInfo")
load("@prelude//apple/validation:required_reasons_tools.bzl", "RequiredReasonsToolsInfo")
load("@prelude//cxx:cxx_toolchain_types.bzl", "CxxPlatformInfo", "CxxToolchainInfo")

def apple_toolchain_impl(ctx: AnalysisContext) -> list[Provider]:
    sdk_path = ctx.attrs._internal_sdk_path or ctx.attrs.sdk_path
    platform_path = ctx.attrs._internal_platform_path or ctx.attrs.platform_path
    dsymutil_cmd = cmd_args(ctx.attrs.dsymutil[RunInfo].args)
    dsymutil_cmd.add(ctx.attrs.dsymutil_flags)

    providers = [
        DefaultInfo(),
        AppleToolchainInfo(
            actool = ctx.attrs.actool[RunInfo],
            app_intents_metadata_processor = ctx.attrs.app_intents_metadata_processor[RunInfo] if ctx.attrs.app_intents_metadata_processor else None,
            app_intents_nl_training_processor = ctx.attrs.app_intents_nl_training_processor[RunInfo] if ctx.attrs.app_intents_nl_training_processor else None,
            architecture = ctx.attrs.architecture,
            codesign = ctx.attrs.codesign[RunInfo],
            codesign_allocate = ctx.attrs.codesign_allocate[RunInfo],
            codesign_identities_command = ctx.attrs.codesign_identities_command[RunInfo] if ctx.attrs.codesign_identities_command else None,
            compile_resources_locally = ctx.attrs.compile_resources_locally,
            copy_scene_kit_assets = ctx.attrs.copy_scene_kit_assets[RunInfo],
            cxx_platform_info = ctx.attrs.cxx_toolchain[CxxPlatformInfo],
            cxx_toolchain_info = ctx.attrs.cxx_toolchain[CxxToolchainInfo],
            dsymutil = RunInfo(args = dsymutil_cmd),
            dwarfdump = ctx.attrs.dwarfdump[RunInfo] if ctx.attrs.dwarfdump else None,
            extra_linker_outputs = ctx.attrs.extra_linker_outputs,
            ibtool = ctx.attrs.ibtool[RunInfo],
            installer = ctx.attrs.installer.label,
            installer_tool = ctx.attrs.installer[RunInfo],
            libtool = ctx.attrs.libtool[RunInfo],
            lipo = ctx.attrs.lipo[RunInfo],
            mapc = ctx.attrs.mapc[RunInfo] if ctx.attrs.mapc else None,
            merge_index_store = ctx.attrs.merge_index_store[RunInfo],
            metal = ctx.attrs.metal[RunInfo] if ctx.attrs.metal else None,
            metallib = ctx.attrs.metallib[RunInfo] if ctx.attrs.metallib else None,
            momc = ctx.attrs.momc[RunInfo],
            objdump = ctx.attrs.objdump[RunInfo] if ctx.attrs.objdump else None,
            platform_path = platform_path,
            required_reasons_tools = ctx.attrs.required_reasons_tools[RequiredReasonsToolsInfo],
            sdk_build_version = ctx.attrs.build_version,
            sdk_canonical_name = ctx.attrs.sdk_canonical_name,
            sdk_name = ctx.attrs.sdk_name,
            sdk_path = sdk_path,
            sdk_version = ctx.attrs.version,
            xcode_build_version = ctx.attrs.xcode_build_version,
            xcode_version = ctx.attrs.xcode_version,
            xctest = ctx.attrs.xctest[RunInfo],
        ),
    ]
    if ctx.attrs.swift_toolchain:
        providers.append(ctx.attrs.swift_toolchain[SwiftToolchainInfo])

    return providers
