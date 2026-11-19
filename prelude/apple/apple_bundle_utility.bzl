# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//utils:utils.bzl", "flatten", "value_or")
load(":apple_bundle_types.bzl", "AppleBundleLinkerMapInfo", "AppleMinDeploymentVersionInfo")
load(":apple_execution_groups.bzl", "AppleResources")
load(":apple_library.bzl", "AppleLibraryForDistributionInfo")
load(":apple_resource_types.bzl", "AppleResourceProcessingOptions")
load(":apple_target_sdk_version.bzl", "get_min_deployment_version_for_node")
load(":apple_toolchain_types.bzl", "AppleToolchainInfo")
load(":resource_groups.bzl", "ResourceGraphInfo")

# `ctx` in all functions below is expected to be of `apple_bundle` or `apple_test` rule

def get_product_name(ctx: AnalysisContext) -> str:
    if hasattr(ctx.attrs, "product_name") and ctx.attrs.product_name != None:
        return ctx.attrs.product_name
    if getattr(ctx.attrs, "product_name_from_module_name", False):
        apple_library_info = get_default_binary_dep(ctx.attrs.binary).get(AppleLibraryForDistributionInfo)
        if apple_library_info != None:
            return apple_library_info.module_name
    return ctx.attrs.name

def get_extension_attr(ctx: AnalysisContext) -> typing.Any:
    return ctx.attrs.extension

def get_default_binary_dep(binary_deps: [dict[str, Dependency], Dependency, None]) -> [Dependency, None]:
    if not type(binary_deps) == "dict":
        return binary_deps

    if len(binary_deps.items()) == 1:
        return binary_deps.values()[0]

    return binary_deps["arm64"] if "arm64" in binary_deps else binary_deps["x86_64"]

def get_flattened_binary_deps(binary_deps: dict[str, Dependency]) -> list[Dependency]:
    return binary_deps.values()

# Derives the effective deployment target for the bundle. It's
# usually the deployment target of the binary if present,
# otherwise it falls back to other values (see implementation).
def get_bundle_min_target_version(ctx: AnalysisContext, binary_or_binaries: [dict[str, Dependency], Dependency, None]) -> str:
    binary = get_default_binary_dep(binary_or_binaries)

    binary_min_version = None

    # apple_xcuitest bundles do not have a binary
    if binary != None:
        min_version_info = binary[AppleMinDeploymentVersionInfo] if AppleMinDeploymentVersionInfo in binary else None
        if min_version_info != None:
            binary_min_version = min_version_info.version

    fallback_min_version = get_min_deployment_version_for_node(ctx)
    min_version = binary_min_version or fallback_min_version

    if min_version != None:
        return min_version

    # TODO(T110378109): support default value from SDK `Info.plist`
    fail("Could not determine min target sdk version for bundle: {}".format(ctx.label))

def get_resources_toolchain(ctx: AnalysisContext) -> Dependency:
    # `apple_bundle()` and `apple_test()` compile resources with the macOS hosted resources
    # toolchain; other rules processing resources (e.g. `apple_xcuitest()`) use `_apple_toolchain`.
    # Every toolchain value read while compiling resources or building the Info.plist comes from it,
    # except the minimum OS version, which is the bundle's own (`get_bundle_min_target_version`).
    return getattr(ctx.attrs, "_apple_resources_toolchain", ctx.attrs._apple_toolchain)

def get_resources_toolchain_info(ctx: AnalysisContext) -> AppleToolchainInfo:
    return get_resources_toolchain(ctx)[AppleToolchainInfo]

def get_resources_sdk_name(ctx: AnalysisContext) -> str:
    return get_resources_toolchain_info(ctx).sdk_name

def get_is_watch_bundle(ctx: AnalysisContext) -> bool:
    return get_resources_sdk_name(ctx).startswith("watch")

def get_resources_execution_group(ctx: AnalysisContext) -> [ExecGroup, None]:
    return AppleResources if hasattr(ctx.attrs, "_apple_resources_toolchain") else None

def get_bundle_resource_processing_options(ctx: AnalysisContext) -> AppleResourceProcessingOptions:
    compile_resources_locally = value_or(ctx.attrs._compile_resources_locally_override, get_resources_toolchain_info(ctx).compile_resources_locally)
    is_watch_bundle = get_is_watch_bundle(ctx)
    return AppleResourceProcessingOptions(
        prefer_local = compile_resources_locally and (not is_watch_bundle),
        # TODO: Remote execution preference should be part of `apple_toolchain()`, same as `compile_resources_locally`
        prefer_remote = is_watch_bundle,
        allow_cache_upload = compile_resources_locally,
    )

def get_bundle_infos_from_graph(graph: ResourceGraphInfo) -> list[AppleBundleLinkerMapInfo]:
    bundle_infos = []
    for node in graph.nodes.traverse():
        if not node.resource_spec:
            continue

        resource_spec = node.resource_spec
        for artifact in resource_spec.files:
            if not isinstance(artifact, Dependency):
                continue

            bundle_info = artifact.get(AppleBundleLinkerMapInfo)
            if bundle_info:
                bundle_infos.append(bundle_info)

    return bundle_infos

def merge_bundle_linker_maps_info(infos: list[AppleBundleLinkerMapInfo]) -> AppleBundleLinkerMapInfo:
    return AppleBundleLinkerMapInfo(
        linker_maps = flatten([info.linker_maps for info in infos]),
    )

def get_apple_versioned_macos_bundle_value_primitive(name: str, versioned_macos_bundle: bool) -> bool:
    # versioned_macos_bundle only makes sense on mac, disregard value otherwise
    if name == "macosx" or name == "maccatalyst":
        return versioned_macos_bundle
    else:
        return False
