# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:artifact_tset.bzl", "make_artifact_tset")
load("@prelude//:validation_deps.bzl", "get_validation_deps_outputs")
load("@prelude//apple:apple_library.bzl", "AppleLibraryForDistributionInfo")
load("@prelude//apple:apple_library_types.bzl", "AppleLibraryInfo")
load("@prelude//apple:apple_toolchain_types.bzl", "AppleToolchainInfo", "AppleToolsInfo")
load("@prelude//apple:xcassets_asset_symbols.bzl", "xcassets_asset_symbol_usage_from_deps")
load("@prelude//cxx:cxx_context.bzl", "get_cxx_toolchain_info")
load("@prelude//linking:link_info.bzl", "LinkStrategy", "get_link_args_for_strategy", "unpack_link_args_object_files_and_lazy_archives_only")
load("@prelude//linking:linkables.bzl", "linkables")
load("@prelude//utils:arglike.bzl", "ArgLike")

def apple_static_archive_impl(ctx: AnalysisContext) -> list[Provider]:
    libtool = ctx.attrs._apple_toolchain[AppleToolchainInfo].libtool
    static_archive_linker = ctx.attrs._apple_tools[AppleToolsInfo].static_archive_linker
    archive_name = ctx.attrs.name if ctx.attrs.archive_name == None else ctx.attrs.archive_name
    output = ctx.actions.declare_output(archive_name, has_content_based_path = False)

    link_args = _get_static_link_args(ctx)
    validation_deps_outputs = get_validation_deps_outputs(ctx)

    # TODO(T193127271): Support thin archives
    cmd = cmd_args([static_archive_linker, "--libtool", libtool, "--output", output.as_output(), link_args], hidden = validation_deps_outputs or [])
    ctx.actions.run(cmd, category = "static_archive_linker", identifier = output.short_path)

    providers = [DefaultInfo(default_output = output), _get_apple_library_info(ctx)] + _get_apple_library_for_distribution_info(ctx)
    providers += xcassets_asset_symbol_usage_from_deps(ctx, ctx.attrs.deps + filter(None, ctx.attrs.flat_deps + [ctx.attrs.distribution_flat_dep]))

    return providers

def _get_apple_library_for_distribution_info(ctx: AnalysisContext) -> list[AppleLibraryForDistributionInfo]:
    if ctx.attrs.distribution_flat_dep != None:
        apple_library_for_distribution = ctx.attrs.distribution_flat_dep.get(AppleLibraryForDistributionInfo)
        if apple_library_for_distribution != None:
            return [apple_library_for_distribution]
    return []

def _get_apple_library_info(ctx: AnalysisContext) -> AppleLibraryInfo:
    all_flat_deps = filter(None, ctx.attrs.flat_deps + [ctx.attrs.distribution_flat_dep])
    flat_apple_library_infos = filter(None, [dep.get(AppleLibraryInfo) for dep in all_flat_deps])
    flat_public_framework_headers = []
    for apple_library_info in flat_apple_library_infos:
        tset = apple_library_info.public_framework_headers._tset
        if tset != None and tset.value:
            for headers in tset.value:
                flat_public_framework_headers += headers.artifacts

    flat_header_tset = make_artifact_tset(
        actions = ctx.actions,
        label = ctx.label,
        artifacts = flat_public_framework_headers,
    )

    apple_library_infos = filter(None, [dep.get(AppleLibraryInfo) for dep in ctx.attrs.deps])
    public_framework_header_tset = make_artifact_tset(
        actions = ctx.actions,
        label = ctx.label,
        children = [apple_library.public_framework_headers for apple_library in apple_library_infos] + [flat_header_tset],
    )

    swift_header = None
    if ctx.attrs.distribution_flat_dep != None:
        distribution_flat_dep_apple_library_info = ctx.attrs.distribution_flat_dep.get(AppleLibraryInfo)
        if distribution_flat_dep_apple_library_info:
            swift_header = distribution_flat_dep_apple_library_info.swift_header

    return AppleLibraryInfo(
        public_framework_headers = public_framework_header_tset,
        swift_header = swift_header,
        target = ctx.label,
        labels = ctx.attrs.labels,
    )

def _get_static_link_args(ctx: AnalysisContext) -> list[ArgLike]:
    args = []

    for dep in ctx.attrs.flat_deps:
        default_info = dep.get(DefaultInfo)
        if default_info == None:
            continue
        default_outputs = default_info.default_outputs
        if len(default_outputs) > 0:
            args.append(default_outputs[0])

    if ctx.attrs.distribution_flat_dep:
        default_info = ctx.attrs.distribution_flat_dep.get(DefaultInfo)
        if default_info != None:
            default_outputs = default_info.default_outputs
            if len(default_outputs) > 0:
                args.append(default_outputs[0])

    args = dedupe(args)

    transitive_link_args = get_link_args_for_strategy(
        ctx.actions,
        ctx.label,
        get_cxx_toolchain_info(ctx).linker_info,
        [x.merged_link_info for x in linkables(ctx.attrs.deps)],
        LinkStrategy("static"),
        prefer_stripped = False,
        transformation_spec_context = None,
    )
    args.append(unpack_link_args_object_files_and_lazy_archives_only(transitive_link_args))

    return args
