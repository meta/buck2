# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//cxx:cxx_toolchain_types.bzl", "CxxInternalTools", "DistLtoToolsInfo")

def _cxx_internal_tools_impl(ctx: AnalysisContext) -> list[Provider]:
    return [
        DefaultInfo(),
        CxxInternalTools(
            check_nonempty_output = ctx.attrs.check_nonempty_output[RunInfo],
            clang_tidy_wrapper = ctx.attrs.clang_tidy_wrapper[RunInfo],
            concatenate_diagnostics = ctx.attrs.concatenate_diagnostics[RunInfo],
            dep_file_processor = ctx.attrs.dep_file_processor[RunInfo],
            dist_lto = ctx.attrs.dist_lto[DistLtoToolsInfo],
            filter_argsfile = ctx.attrs.filter_argsfile[RunInfo],
            hmap_wrapper = ctx.attrs.hmap_wrapper[RunInfo],
            make_comp_db = ctx.attrs.make_comp_db[RunInfo],
            remap_cwd = ctx.attrs.remap_cwd[RunInfo],
            serialized_diagnostics_to_json_wrapper = ctx.attrs.serialized_diagnostics_to_json_wrapper[RunInfo],
            shlib_interface_link_wrapper = ctx.attrs.shlib_interface_link_wrapper[RunInfo],
            stderr_to_file = ctx.attrs.stderr_to_file[RunInfo],
            stub_header_unit = ctx.attrs.stub_header_unit[RunInfo],
        ),
    ]

cxx_internal_tools = rule(
    impl = _cxx_internal_tools_impl,
    attrs = {
        "check_nonempty_output": attrs.dep(providers = [RunInfo], default = "prelude//cxx/tools:check_nonempty_output"),
        "clang_tidy_wrapper": attrs.dep(providers = [RunInfo], default = "prelude//cxx/tools:clang_tidy_wrapper"),
        "concatenate_diagnostics": attrs.dep(providers = [RunInfo], default = "prelude//cxx/tools:concatenate_diagnostics"),
        "dep_file_processor": attrs.dep(providers = [RunInfo], default = "prelude//cxx/tools:dep_file_processor"),
        "dist_lto": attrs.dep(providers = [DistLtoToolsInfo], default = "prelude//cxx/dist_lto/tools:dist_lto_tools"),
        "filter_argsfile": attrs.dep(providers = [RunInfo], default = "prelude//cxx/tools:filter_argsfile"),
        "hmap_wrapper": attrs.dep(providers = [RunInfo], default = "prelude//cxx/tools:hmap_wrapper"),
        "make_comp_db": attrs.dep(providers = [RunInfo], default = "prelude//cxx/tools:make_comp_db"),
        "remap_cwd": attrs.dep(providers = [RunInfo], default = "prelude//cxx/tools:remap_cwd"),
        "serialized_diagnostics_to_json_wrapper": attrs.dep(providers = [RunInfo], default = "prelude//cxx/tools:serialized_diagnostics_to_json_wrapper"),
        "shlib_interface_link_wrapper": attrs.dep(providers = [RunInfo], default = "prelude//cxx/tools:shlib_interface_link_wrapper"),
        "stderr_to_file": attrs.dep(providers = [RunInfo], default = "prelude//cxx/tools:stderr_to_file"),
        "stub_header_unit": attrs.dep(providers = [RunInfo], default = "prelude//cxx/tools:stub_header_unit"),
    },
)

def _cxx_hacks_impl(_ctx):
    return [
        DefaultInfo(),
        TemplatePlaceholderInfo(
            unkeyed_variables = {
                "cxx-header-tree": "/dev/null/HACK-CXX-HEADER-TREE",
            },
        ),
    ]

cxx_hacks = rule(
    impl = _cxx_hacks_impl,
    attrs = {},
)
