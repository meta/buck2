# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//cxx:cxx_toolchain_types.bzl", "DistLtoToolsInfo", "LinkerType")

def _impl(ctx):
    return [
        DefaultInfo(),
        DistLtoToolsInfo(
            compiler_stats_merger = ctx.attrs.compiler_stats_merger[RunInfo],
            planner = {LinkerType(linker_type): planner[RunInfo] for linker_type, planner in ctx.attrs.planner.items()},
            opt = {LinkerType(linker_type): opt[RunInfo] for linker_type, opt in ctx.attrs.opt.items()},
            prepare = {LinkerType(linker_type): prepare[RunInfo] for linker_type, prepare in ctx.attrs.prepare.items()},
            copy = ctx.attrs.copy[RunInfo],
            archive_mapper = ctx.attrs.archive_mapper[RunInfo],
        ),
    ]

dist_lto_tools = rule(
    impl = _impl,
    attrs = {
        "archive_mapper": attrs.dep(providers = [RunInfo], default = "prelude//cxx/dist_lto/tools:dist_lto_archive_mapper"),
        "compiler_stats_merger": attrs.dep(providers = [RunInfo], default = "prelude//cxx/dist_lto/tools:dist_lto_compiler_stats_merger"),
        "copy": attrs.dep(providers = [RunInfo], default = "prelude//cxx/dist_lto/tools:dist_lto_copy"),
        "opt": attrs.dict(
            key = attrs.enum(LinkerType.values()),
            value = attrs.dep(providers = [RunInfo]),
            default = {
                "darwin": "prelude//cxx/dist_lto/tools:dist_lto_opt_darwin",
                "gnu": "prelude//cxx/dist_lto/tools:dist_lto_opt_gnu",
            },
        ),
        "planner": attrs.dict(
            key = attrs.enum(LinkerType.values()),
            value = attrs.dep(providers = [RunInfo]),
            default = {
                "darwin": "prelude//cxx/dist_lto/tools:dist_lto_planner_darwin",
                "gnu": "prelude//cxx/dist_lto/tools:dist_lto_planner_gnu",
            },
        ),
        "prepare": attrs.dict(
            key = attrs.enum(LinkerType.values()),
            value = attrs.dep(providers = [RunInfo]),
            default = {
                "darwin": "prelude//cxx/dist_lto/tools:dist_lto_prepare_darwin",
                "gnu": "prelude//cxx/dist_lto/tools:dist_lto_prepare_gnu",
            },
        ),
    },
)
