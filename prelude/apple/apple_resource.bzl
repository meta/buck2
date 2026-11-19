# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    "@prelude//ide_integrations/xcode:data.bzl",
    "XCODE_DATA_SUB_TARGET",
    "XcodeDataInfoKeys",
    "generate_xcode_data",
)
load(":apple_resource_types.bzl", "AppleResourceDestination", "AppleResourceSpec")
load(":resource_groups.bzl", "create_resource_graph")

def _xcode_populate_attributes(ctx) -> dict[str, typing.Any]:
    extra_xcode_files = []

    # ctx.attrs.files can contain Dependency
    for file in ctx.attrs.files:
        if isinstance(file, Dependency):
            extra_xcode_files.extend(file[DefaultInfo].default_outputs)
        else:
            extra_xcode_files.append(file)

    extra_xcode_files += ctx.attrs.dirs + ctx.attrs.variants

    # Named varients map a str to a set, so we need to add the values from the set
    for val in ctx.attrs.named_variants.values():
        extra_xcode_files.extend(val)

    data = {}
    if extra_xcode_files:
        data[XcodeDataInfoKeys.EXTRA_XCODE_FILES] = extra_xcode_files

    return data

def apple_resource_impl(ctx: AnalysisContext) -> list[Provider]:
    destination = ctx.attrs.destination or "resources"
    resource_spec = AppleResourceSpec(
        files = ctx.attrs.files,
        dirs = ctx.attrs.dirs,
        content_dirs = ctx.attrs.content_dirs,
        destination = AppleResourceDestination(destination),
        variant_files = ctx.attrs.variants or [],
        named_variant_files = ctx.attrs.named_variants or {},
        codesign_files_on_copy = ctx.attrs.codesign_on_copy,
        codesign_entitlements = ctx.attrs.codesign_entitlements,
        codesign_flags_override = ctx.attrs.codesign_flags_override,
    )

    # `files` can contain `apple_library()` which in turn can have `apple_resource()` deps
    file_deps = [file_or_dep for file_or_dep in ctx.attrs.files if isinstance(file_or_dep, Dependency)]
    deps = file_deps + ctx.attrs.resources_from_deps
    graph = create_resource_graph(
        ctx = ctx,
        labels = ctx.attrs.labels,
        deps = deps,
        exported_deps = [],
        resource_spec = resource_spec,
    )
    xcode_data_default_info, xcode_data_info = generate_xcode_data(ctx, "apple_resource", None, _xcode_populate_attributes)

    return [
        DefaultInfo(
            sub_targets = {
                "headers": [
                    DefaultInfo(default_outputs = []),
                ],
                XCODE_DATA_SUB_TARGET: xcode_data_default_info,
            },
        ),
        graph,
        xcode_data_info,
    ]
