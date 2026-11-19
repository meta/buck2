# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

XPluginsManifestInfo = provider(
    fields = {
        "function_mapping": provider_field(Artifact),
        "manifest": provider_field(Artifact),
    }
)

XPluginsDebugArtifactsEntry = record(
    target = field(Label),
    manifest_info = field(XPluginsManifestInfo),
)

XPluginsDebugArtifactsTSet = transitive_set()

XPluginsDebugArtifactsInfo = provider(
    fields = {
        "tset": provider_field(XPluginsDebugArtifactsTSet),
    }
)

XPluginsFunctionMappingManifestInfo = provider(
    fields = {
        "manifest": provider_field(Artifact),
    }
)

XPluginsPluginUsageInfo = provider(
    fields = {
        "target": provider_field(Label),
        "usage_info": provider_field(Artifact),
    }
)

XPluginsSocketUsageInfo = provider(
    fields = {
        "target": provider_field(Label),
        "usage_info": provider_field(Artifact),
    }
)

def _get_artifacts(values: list[XPluginsPluginUsageInfo | XPluginsSocketUsageInfo]) -> list[Artifact]:
    return [v.usage_info for v in values]

XPluginsUsageInfoSet = transitive_set(
    args_projections = {
        "artifacts": _get_artifacts,
    },
)

XPluginsUsageInfo = provider(
    fields = {
        "plugin_info_tset": provider_field(XPluginsUsageInfoSet),
        "socket_info_tset": provider_field(XPluginsUsageInfoSet),
    }
)
