# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    "@prelude//:artifact_tset.bzl",
    "ArtifactInfoTag",
    "ArtifactTSet",
    "make_artifact_tset",
)
load(":swift_pcm_compilation_types.bzl", "WrappedSwiftPCMCompiledInfo")
load(":swift_toolchain.bzl", "get_swift_toolchain_info")
load(
    ":swift_toolchain_types.bzl",
    "WrappedSdkCompiledModuleInfo",
)

def extract_and_merge_swift_debug_infos(ctx: AnalysisContext, compiled_pcm_deps_providers, artifacts: list[Artifact] = []) -> ArtifactTSet:
    provide_swift_debug_info = get_swift_toolchain_info(ctx).provide_swift_debug_info
    swift_debug_tsets = (
        [
            d[WrappedSdkCompiledModuleInfo].swift_debug_info
            for d in compiled_pcm_deps_providers
            if WrappedSdkCompiledModuleInfo in d and d[WrappedSdkCompiledModuleInfo].swift_debug_info != None
        ]
        if provide_swift_debug_info
        else []
    )

    return make_artifact_tset(
        actions = ctx.actions,
        label = ctx.label,
        artifacts = artifacts if provide_swift_debug_info else [],
        children = swift_debug_tsets,
        tags = [ArtifactInfoTag("swift_debug_info")],
    )

def extract_and_merge_clang_debug_infos(ctx: AnalysisContext, compiled_pcm_deps_providers, pcms_and_modulemaps: list[Artifact] = []) -> ArtifactTSet:
    clang_debug_tsets = []
    for d in compiled_pcm_deps_providers:
        if WrappedSdkCompiledModuleInfo in d and d[WrappedSdkCompiledModuleInfo].clang_debug_info != None:
            clang_debug_tsets.append(d[WrappedSdkCompiledModuleInfo].clang_debug_info)
        if WrappedSwiftPCMCompiledInfo in d:
            clang_debug_tsets.append(d[WrappedSwiftPCMCompiledInfo].clang_debug_info)

    swift_toolchain = get_swift_toolchain_info(ctx)
    swift_toolchain_debug_info_tsets = [swift_toolchain.sdk_debug_info] if swift_toolchain.sdk_debug_info else []

    return make_artifact_tset(
        actions = ctx.actions,
        label = ctx.label,
        # Even with explicit modules, lldb requires the presence of the modulemaps.
        artifacts = pcms_and_modulemaps,
        children = clang_debug_tsets + swift_toolchain_debug_info_tsets,
        tags = [ArtifactInfoTag("swift_debug_info")],
    )
