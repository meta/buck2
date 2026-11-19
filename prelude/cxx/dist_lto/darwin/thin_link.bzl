# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    "@prelude//linking:link_info.bzl",
    "append_linkable_args",
)
load(":common_types.bzl", "DThinLTOLinkData", "LinkDataType")

def thin_link(
    ctx: AnalysisContext,
    final_binary_out: Artifact,
    sorted_index_link_data: list[DThinLTOLinkData],
    common_link_flags: cmd_args,
    linker: RunInfo,
    lto_planner: RunInfo,
    post_linker_flags: cmd_args,
    index_argsfile_out: Artifact,
    link_plan_out: Artifact,
    identifier: str | None,
    use_content_based_output_paths: bool,
    make_cat,
    distributed_link_action_identifier: str,
):
    # See comments in dist_lto_planner.py for semantics on the values that are pushed into index_meta.
    index_meta_records = []

    index_cat = make_cat("thin_lto_index")
    index_path = "{}/{}".format(distributed_link_action_identifier, index_cat)
    index_file_out = ctx.actions.declare_output(index_path + "/index", has_content_based_path = use_content_based_output_paths)
    index_out_dir = cmd_args(index_file_out.as_output(), parent = 1)

    index_args = cmd_args(common_link_flags)
    index_args.add(cmd_args(index_file_out.as_output(), format = "-Wl,--thinlto-index-only={}"))
    index_args.add("-Wl,--thinlto-emit-imports-files")
    index_args.add("-Wl,--thinlto-full-index")

    # By default the linker will write artifacts (import files and sharded indices) next to input bitcode files with
    # a different suffix. This can be problematic if you are running two distributed links on the same machine at the # same time consuming the same input bitcode files. That is the links would overwrite each other's artifacts. This
    # flag allows you to write all these artifacts into a unique directory per link to avoid this problem.
    index_args.add(cmd_args(index_out_dir, format = "-Wl,--thinlto-prefix-replace=;{}/"))

    # buildifier: disable=uninitialized
    for idx, artifact in enumerate(sorted_index_link_data):
        link_data = artifact.link_data

        if artifact.data_type == LinkDataType("eager_bitcode") or artifact.data_type == LinkDataType("lazy_bitcode"):
            if artifact.data_type == LinkDataType("lazy_bitcode") and link_data.archive_start:
                index_args.add("-Wl,--start-lib")

            index_args.add(link_data.input_object_file)

            if artifact.data_type == LinkDataType("lazy_bitcode") and link_data.archive_end:
                index_args.add("-Wl,--end-lib")

            object_file_record = {
                "input_object_file_path": link_data.input_object_file,
                "output_index_shard_file_path": link_data.output_index_shard_file.as_output(),
                "output_plan_file_path": link_data.plan.as_output(),
                "starlark_array_index": idx,
            }
            index_meta_records.append(object_file_record)

        elif artifact.data_type == LinkDataType("dynamic_library"):
            append_linkable_args(index_args, link_data.linkable)

        else:
            fail("Unhandled data type: {}".format(str(artifact.data_type)))

    output_as_string = cmd_args(final_binary_out, ignore_artifacts = True)
    index_args.add("-o", output_as_string)
    index_args.add(post_linker_flags)
    index_cmd = cmd_args(linker)
    index_argfile, _ = ctx.actions.write(
        index_argsfile_out.as_output(),
        index_args,
        allow_args = True,
    )
    index_cmd.add(cmd_args(index_argfile, format = "@{}"))

    index_meta_file = ctx.actions.write_json(
        final_binary_out.basename + ".thinlto.meta.json",
        index_meta_records,
        with_inputs = True,
        has_content_based_path = use_content_based_output_paths,
    )

    plan_cmd = cmd_args([lto_planner, "--meta", index_meta_file, "--index", index_out_dir, "--link-plan", link_plan_out.as_output()])
    plan_cmd.add("--", index_cmd)

    plan_cmd.add(
        cmd_args(
            hidden = [
                index_args,
            ]
        )
    )
    ctx.actions.run(plan_cmd, category = index_cat, identifier = identifier)
