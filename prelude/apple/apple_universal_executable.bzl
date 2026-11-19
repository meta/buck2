# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    "@prelude//:artifact_tset.bzl",
    "project_artifacts",
)
load(":apple_bundle_types.bzl", "AppleBundleLinkerMapInfo", "AppleMinDeploymentVersionInfo")
load(":apple_bundle_utility.bzl", "get_default_binary_dep", "get_flattened_binary_deps", "merge_bundle_linker_maps_info")
load(":apple_code_signing_types.bzl", "AppleEntitlementsInfo")
load(":apple_dsym.bzl", "DSYM_SUBTARGET", "get_apple_dsym_ext")
load(":apple_universal_binaries.bzl", "create_universal_binary", "get_universal_binary_name")
load(":debug.bzl", "AppleDebuggableInfo", "DEBUGINFO_SUBTARGET")
load(":resource_groups.bzl", "ResourceGraphInfo")
load(":xcassets_asset_symbols.bzl", "xcassets_asset_symbol_usage_from_deps")

_FORWARDED_PROVIDER_TYPES = [
    AppleMinDeploymentVersionInfo,
    AppleEntitlementsInfo,
    ResourceGraphInfo,
]

_MERGED_PROVIDER_TYPES = [
    AppleDebuggableInfo,
    AppleBundleLinkerMapInfo,
]

def apple_universal_executable_impl(ctx: AnalysisContext) -> list[Provider]:
    dsym_name = ctx.attrs.name + ".dSYM"
    binary_outputs = create_universal_binary(
        ctx = ctx,
        binary_deps = ctx.attrs.executable,
        binary_name = get_universal_binary_name(ctx),
        dsym_bundle_name = dsym_name,
        split_arch_dsym = ctx.attrs.split_arch_dsym,
    )

    debug_info = project_artifacts(
        actions = ctx.actions,
        tsets = binary_outputs.debuggable_info.debug_info_tset,
    )

    sub_targets = {}
    if ctx.attrs.split_arch_dsym:
        dsyms = binary_outputs.debuggable_info.dsyms
    else:
        dsyms = [
            get_apple_dsym_ext(
                ctx = ctx,
                executable = binary_outputs.binary,
                debug_info = debug_info,
                action_identifier = ctx.attrs.name + "_dsym",
                output_path = dsym_name,
            )
        ]
    sub_targets[DSYM_SUBTARGET] = [DefaultInfo(default_outputs = dsyms)]

    debug_info_artifacts_manifest = ctx.actions.write(
        "debuginfo.artifacts",
        debug_info,
        with_inputs = True,
        has_content_based_path = False,
    )
    sub_targets[DEBUGINFO_SUBTARGET] = [DefaultInfo(default_output = debug_info_artifacts_manifest)]

    default_binary = get_default_binary_dep(ctx.attrs.executable)
    forwarded_providers = []
    for forward_provider_type in _FORWARDED_PROVIDER_TYPES:
        provider = default_binary.get(forward_provider_type)
        if provider != None:
            forwarded_providers += [provider]

    merged_providers = []
    all_binarys = get_flattened_binary_deps(ctx.attrs.executable)
    for merged_provider_type in _MERGED_PROVIDER_TYPES:
        if default_binary.get(merged_provider_type) == None:
            continue
        if merged_provider_type == AppleDebuggableInfo:
            merged_providers += [
                AppleDebuggableInfo(
                    dsyms = dsyms,
                    binaries = binary_outputs.debuggable_info.binaries,
                    debug_info_tset = binary_outputs.debuggable_info.debug_info_tset,
                ),
            ]
        elif merged_provider_type == AppleBundleLinkerMapInfo:
            merged_providers += [
                merge_bundle_linker_maps_info([binary[AppleBundleLinkerMapInfo] for binary in all_binarys]),
            ]
        else:
            fail("Unhandled provider type: {}".format(merged_provider_type))

    return (
        [
            DefaultInfo(default_output = binary_outputs.binary, sub_targets = sub_targets),
            RunInfo(args = cmd_args(binary_outputs.binary)),
        ]
        + forwarded_providers
        + merged_providers
        + xcassets_asset_symbol_usage_from_deps(ctx, all_binarys)
    )
