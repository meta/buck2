# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//cxx:cxx_apple_linker_flags.bzl", "apple_format_metal_target_triple", "apple_format_target_triple", "apple_target_triple_flags")
load("@prelude//cxx:cxx_context.bzl", "get_cxx_platform_info", "get_cxx_toolchain_info")

def version_is_greater(left: str, right: str) -> bool:
    # Assumes version strings are in dotted format 1.2.4.
    # After comparing components the longer remainder is
    # considered larger.
    left_components = left.split(".")
    right_components = right.split(".")
    for pair in zip(left_components, right_components):
        x = int(pair[0])
        y = int(pair[1])
        if x < y:
            return False
        elif x > y:
            return True

    return len(left_components) > len(right_components)

def get_target_sdk_version(ctx: AnalysisContext) -> [None, str]:
    if not (hasattr(ctx.attrs, "_cxx_toolchain") or hasattr(ctx.attrs, "_apple_toolchain")):
        return None

    toolchain_target_sdk_version = get_cxx_toolchain_info(ctx).minimum_os_version
    target_sdk_version = getattr(ctx.attrs, "target_sdk_version", None)
    if toolchain_target_sdk_version == None:
        if target_sdk_version == None:
            return None
        # There should never be cases where a target has `target_sdk_version`
        # without the toolchain having a version as well.
        fail(
            "Found target which sets `target_sdk_version` for Apple platforms without the toolchain having a `minimum_os_version`, this results in undefined behavior at runtime"
        )

    if target_sdk_version and version_is_greater(target_sdk_version, toolchain_target_sdk_version):
        # The requested target_sdk_version on the toolchain must be >=
        # the version set on the target, which should be the minimum
        # allowed for this version to build.
        fail(
            "{} has target_sdk_version {}, which is larger than the toolchain target_sdk_version of {}".format(
                ctx.label,
                target_sdk_version,
                toolchain_target_sdk_version,
            )
        )

    return toolchain_target_sdk_version

def _format_target_triple(ctx: AnalysisContext, version: str) -> str:
    platform_info = get_cxx_platform_info(ctx)
    return apple_format_target_triple(platform_info.name, version)

def get_target_triple(ctx: AnalysisContext) -> [None, str]:
    target_sdk_version = get_target_sdk_version(ctx)
    if target_sdk_version == None:
        return None

    return _format_target_triple(ctx, target_sdk_version)

def get_versioned_metal_target_triple(ctx: AnalysisContext, version: str) -> str:
    platform_info = get_cxx_platform_info(ctx)
    return apple_format_metal_target_triple(platform_info.name, version)

def get_unversioned_target_triple(ctx: AnalysisContext) -> str:
    return _format_target_triple(ctx, "")

def get_target_sdk_version_flags(ctx: AnalysisContext) -> list[str]:
    target_triple = get_target_triple(ctx)
    return apple_target_triple_flags(target_triple)
