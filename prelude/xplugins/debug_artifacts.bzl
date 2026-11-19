# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    ":types.bzl",
    "XPluginsDebugArtifactsEntry",
    "XPluginsDebugArtifactsInfo",
    "XPluginsDebugArtifactsTSet",
    "XPluginsFunctionMappingManifestInfo",
    "XPluginsManifestInfo",
)

def xplugins_get_debug_artifacts_info(ctx: AnalysisContext, deps: list[Dependency]) -> XPluginsDebugArtifactsInfo | None:
    children = []
    for dep in deps:
        info = dep.get(XPluginsDebugArtifactsInfo)
        if info:
            children.append(info.tset)
    value = None

    # We don't expose this on apple_bundle because it should be propagated from the binary.
    manifest_dep = getattr(ctx.attrs, "xplugins_debug_artifacts", None)
    manifest_info = manifest_dep.get(XPluginsManifestInfo) if manifest_dep else None
    if manifest_info:
        value = XPluginsDebugArtifactsEntry(
            target = ctx.label,
            manifest_info = manifest_info,
        )
    if value or children:
        tset = ctx.actions.tset(
            XPluginsDebugArtifactsTSet,
            value = value,
            children = children,
        )
        return XPluginsDebugArtifactsInfo(tset = tset)
    return None

def xplugins_get_function_mapping_manifest_info(
    actions: AnalysisActions, app_target: Label | None, info: XPluginsDebugArtifactsInfo | None
) -> XPluginsFunctionMappingManifestInfo:
    if not info:
        info = XPluginsDebugArtifactsInfo(tset = actions.tset(XPluginsDebugArtifactsTSet))

    function_mappings = []
    function_mapping_artifacts = []
    function_mapping_targets = []

    for entry in info.tset.traverse():
        if entry:
            function_mapping_artifacts.append(entry.manifest_info.function_mapping)
            function_mapping_targets.append(entry.target)
            function_mappings.append({
                "path": entry.manifest_info.function_mapping,
                "target": entry.target,
            })

    if app_target != None and app_target not in function_mapping_targets:
        fail("App target {} is not listed in the XPlugins function mappings: {}".format(app_target, function_mapping_targets))

    function_mapping_manifest_file = actions.write_json(
        "function_mapping_manifest.json",
        {
            "app_target": app_target,
            "mappings": function_mappings,
        },
        has_content_based_path = False,
        pretty = True,
    ).with_associated_artifacts(function_mapping_artifacts)

    return XPluginsFunctionMappingManifestInfo(
        manifest = function_mapping_manifest_file,
    )

def xplugins_get_debug_artifacts_subtargets(info: XPluginsFunctionMappingManifestInfo) -> list[Provider]:
    return [
        DefaultInfo(
            sub_targets = {
                "function_mapping_manifest": [
                    DefaultInfo(
                        default_output = info.manifest,
                    ),
                ],
            },
        ),
    ]
