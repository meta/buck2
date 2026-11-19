# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Attached to a platform toolchain so any supporting rule can reach the tools.

TargetStatsToolsInfo = provider(
    fields = {
        "extract_target_data": provider_field(RunInfo),
        "file_cycles": provider_field(RunInfo),
        "file_stats": provider_field(RunInfo),
        "package_cycles": provider_field(RunInfo),
    },
)

def _target_stats_tools_impl(ctx: AnalysisContext) -> list[Provider]:
    return [
        DefaultInfo(),
        TargetStatsToolsInfo(
            file_cycles = ctx.attrs.file_cycles[RunInfo],
            file_stats = ctx.attrs.file_stats[RunInfo],
            package_cycles = ctx.attrs.package_cycles[RunInfo],
            extract_target_data = ctx.attrs.extract_target_data[RunInfo],
        ),
    ]

target_stats_tools = rule(
    impl = _target_stats_tools_impl,
    attrs = {
        "extract_target_data": attrs.exec_dep(providers = [RunInfo]),
        "file_cycles": attrs.exec_dep(providers = [RunInfo]),
        "file_stats": attrs.exec_dep(providers = [RunInfo]),
        "package_cycles": attrs.exec_dep(providers = [RunInfo]),
    },
)
