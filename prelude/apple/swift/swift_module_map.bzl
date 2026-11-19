# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//apple:apple_toolchain_types.bzl", "AppleToolsInfo")
load("@prelude//utils:arglike.bzl", "ArgLike")  # @unused Used as a type
load(
    ":swift_incremental_support.bzl",
    "get_uses_content_based_paths",
)
load(":swift_toolchain_types.bzl", "SwiftCompiledModuleTset")

def write_swift_module_map_with_deps(ctx: AnalysisContext, module_name: str, all_deps: SwiftCompiledModuleTset) -> (Artifact, ArgLike):
    uses_content_based_paths = get_uses_content_based_paths(ctx)
    artifact = ctx.actions.declare_output(
        module_name + ".swift_module_map.json",
        has_content_based_path = uses_content_based_paths,
    )
    args = ctx.actions.write_json(
        artifact.as_output(),
        all_deps.project_as_json("swift_module_map"),
        pretty = True,
        with_inputs = True,
    )

    # There are cases where we can see duplicate entries in the modulemaps.
    # This happens when there's different execution platforms across
    # `apple_library()` targets. For more info, see `get_swift_interface_anon_targets()`.
    deduplicated_artifact = ctx.actions.declare_output(
        module_name + ".deduplicated.swift_module_map.json",
        has_content_based_path = uses_content_based_paths,
    )
    ctx.actions.run(
        cmd_args(
            ctx.attrs._apple_tools[AppleToolsInfo].dedupe_swift_module_map,
            "--input",
            artifact,
            "--output",
            deduplicated_artifact.as_output(),
        ),
        category = "swift_module_map_dedupe",
        identifier = module_name,
    )

    # The de-duplicated map holds the same module artifact paths as the original,
    # so carry those inputs along for downstream consumers to materialize.
    deduplicated_args = cmd_args(deduplicated_artifact, hidden = args)
    return (deduplicated_artifact, deduplicated_args)
