# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    ":types.bzl",
    "XPluginsPluginUsageInfo",
    "XPluginsSocketUsageInfo",
    "XPluginsUsageInfo",
    "XPluginsUsageInfoSet",
)

def get_xplugins_usage_info(actions: AnalysisActions, all_deps: list[Dependency]) -> XPluginsUsageInfo | None:
    plugin_manifests = [d[XPluginsPluginUsageInfo] for d in all_deps if XPluginsPluginUsageInfo in d]
    socket_manifests = [d[XPluginsSocketUsageInfo] for d in all_deps if XPluginsSocketUsageInfo in d]
    usage_infos = [d[XPluginsUsageInfo] for d in all_deps if XPluginsUsageInfo in d]

    if plugin_manifests or socket_manifests or usage_infos:
        plugin_info_tset = actions.tset(
            XPluginsUsageInfoSet,
            value = plugin_manifests,
            children = [u.plugin_info_tset for u in usage_infos],
        )
        socket_info_tset = actions.tset(
            XPluginsUsageInfoSet,
            value = socket_manifests,
            children = [u.socket_info_tset for u in usage_infos],
        )
        return XPluginsUsageInfo(
            plugin_info_tset = plugin_info_tset,
            socket_info_tset = socket_info_tset,
        )
    else:
        return None
