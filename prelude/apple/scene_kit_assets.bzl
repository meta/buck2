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
load(
    ":apple_bundle_utility.bzl",
    "get_bundle_min_target_version",
    "get_bundle_resource_processing_options",
    "get_resources_execution_group",
    "get_resources_sdk_name",
    "get_resources_toolchain_info",
)
load(":resource_groups.bzl", "create_resource_graph")
load(":scene_kit_assets_types.bzl", "SceneKitAssetsSpec")

def scene_kit_assets_impl(ctx: AnalysisContext) -> list[Provider]:
    spec = SceneKitAssetsSpec(
        path = ctx.attrs.path,
    )
    graph = create_resource_graph(
        ctx = ctx,
        labels = ctx.attrs.labels,
        deps = [],
        exported_deps = [],
        scene_kit_assets_spec = spec,
    )

    xcode_data_default_info, xcode_data_info = generate_xcode_data(ctx, "apple_asset_catalog", None, _xcode_populate_attributes)

    return [
        DefaultInfo(
            sub_targets = {
                XCODE_DATA_SUB_TARGET: xcode_data_default_info,
            },
        ),
        graph,
        xcode_data_info,
    ]

def compile_scene_kit_assets(ctx: AnalysisContext, specs: list[SceneKitAssetsSpec]) -> Artifact | None:
    if len(specs) == 0:
        return None

    output = ctx.actions.declare_output("SceneKitAssetsCompiled", has_content_based_path = False)

    copy_scene_kit_assets_cmds = []
    for spec in specs:
        copy_scene_kit_assets_cmd = _get_copy_scene_kit_assets_cmd(ctx, spec)
        copy_scene_kit_assets_cmds.append(copy_scene_kit_assets_cmd)

    # Sandboxing and fs isolation on RE machines results in Xcode tools failing
    # when those are working in freshly created directories in buck-out.
    # See https://fb.workplace.com/groups/1042353022615812/permalink/1872164996301273/
    # As a workaround create a directory in tmp, use it for Xcode tools, then
    # copy the result to buck-out.
    wrapper_script, _ = ctx.actions.write(
        "copy_scene_kit_assets_wrapper.sh",
        [
            cmd_args("set -euo pipefail"),
            cmd_args('export TMPDIR="$(mktemp -d)"'),
            cmd_args(copy_scene_kit_assets_cmds),
            cmd_args(output, format = 'mkdir -p {} && cp -R "$TMPDIR"/. {}'),
        ],
        allow_args = True,
        has_content_based_path = False,
        exec_group = get_resources_execution_group(ctx),
    )
    combined_command = cmd_args(["/bin/sh", wrapper_script], hidden = copy_scene_kit_assets_cmds + [output.as_output()])
    processing_options = get_bundle_resource_processing_options(ctx)
    ctx.actions.run(
        combined_command,
        prefer_local = processing_options.prefer_local,
        prefer_remote = processing_options.prefer_remote,
        allow_cache_upload = processing_options.allow_cache_upload,
        exec_group = get_resources_execution_group(ctx),
        category = "scene_kit_assets",
    )
    return output

def _get_copy_scene_kit_assets_cmd(ctx: AnalysisContext, scene_kit_assets_spec: SceneKitAssetsSpec) -> cmd_args:
    scnassets_folder = scene_kit_assets_spec.path.basename
    return cmd_args(
        [
            get_resources_toolchain_info(ctx).copy_scene_kit_assets,
            scene_kit_assets_spec.path,
            "-o",
            cmd_args(scnassets_folder, format = "$TMPDIR/{}"),
            "--target-platform=" + get_resources_sdk_name(ctx),
            "--target-version=" + get_bundle_min_target_version(ctx, ctx.attrs.binary),
        ],
        delimiter = " ",
    )

def _xcode_populate_attributes(ctx) -> dict[str, typing.Any]:
    data = {XcodeDataInfoKeys.EXTRA_XCODE_FILES: [ctx.attrs.path]}
    return data
