# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _basic_incremental_actions_impl(ctx) -> list[Provider]:
    out = ctx.actions.declare_output("out", has_content_based_path = ctx.attrs.use_content_based_path)
    ctx.actions.run(
        cmd_args(
            ["fbpython", ctx.attrs.script] + ["--out", out.as_output()],
            hidden = [ctx.actions.write("invalidate_action_and_metadata", ctx.attrs.invalidate, has_content_based_path = False)],
        ),
        category = "incremental",
        no_outputs_cleanup = ctx.attrs.use_incremental,
        metadata_env_var = "METADATA_PATH",
        metadata_path = "metadata.json",
    )
    return [
        DefaultInfo(out),
        RunInfo(args = ["cat", out]),
    ]

basic_incremental_action = rule(
    impl = _basic_incremental_actions_impl,
    attrs = {
        "invalidate": attrs.string(),
        "script": attrs.source(),
        "use_content_based_path": attrs.bool(default = read_config("test", "use_content_based_path", "") in ["true", "True"]),
        "use_incremental": attrs.bool(default = (read_config("test", "use_incremental", True) == True)),
    },
)

def _incremental_action_with_metadata_optout_impl(ctx) -> list[Provider]:
    out = ctx.actions.declare_output("out", has_content_based_path = False)
    input_not_in_metadata = ctx.actions.write("input_not_in_metadata", "input_not_in_metadata", has_content_based_path = False)
    input_in_metadata = ctx.actions.write("input_in_metadata", "input_in_metadata", has_content_based_path = False)

    script = """
import json
import os
import sys

with open(os.environ["METADATA"], "r") as f:
    metadata_digests = json.load(f)['digests']
    assert len(metadata_digests) == 1
    assert "input_in_metadata" in metadata_digests[0]['path']

with open(sys.argv[1], "w") as f:
    f.write("output")
"""
    artifact_tag = ctx.actions.artifact_tag()

    script = artifact_tag.tag_artifacts(ctx.actions.write("script.py", script, is_executable = True, has_content_based_path = False))

    ctx.actions.run(
        cmd_args(
            ["fbpython", script, out.as_output()],
            hidden = [
                artifact_tag.tag_artifacts(input_not_in_metadata),
                artifact_tag.tag_artifacts(input_in_metadata),
                input_in_metadata,
                artifact_tag.tag_artifacts(input_in_metadata),
            ],
        ),
        category = "incremental",
        no_outputs_cleanup = True,
        local_only = True,
        metadata_env_var = "METADATA",
        metadata_path = "metadata.json",
        incremental_metadata_ignore_tags = [artifact_tag],
    )
    return [
        DefaultInfo(out),
    ]

incremental_action_with_metadata_optout = rule(
    impl = _incremental_action_with_metadata_optout_impl,
    attrs = {},
)

def _incremental_action_with_multiple_outputs_impl(ctx) -> list[Provider]:
    out1 = ctx.actions.declare_output("out1", has_content_based_path = ctx.attrs.use_content_based_path)
    out2 = ctx.actions.declare_output("out2", has_content_based_path = ctx.attrs.use_content_based_path)
    ctx.actions.run(
        cmd_args(["fbpython", ctx.attrs.script] + ["--out1", out1.as_output(), "--out2", out2.as_output()]),
        category = "incremental",
        no_outputs_cleanup = True,
        env = {"INVALIDATE_ACTION": ctx.attrs.invalidate},
    )
    return [
        DefaultInfo(default_outputs = [out1], sub_targets = {"out2": [DefaultInfo(default_outputs = [out2])]}),
        RunInfo(args = ["cat", out1, out2]),
    ]

incremental_action_with_multiple_outputs = rule(
    impl = _incremental_action_with_multiple_outputs_impl,
    attrs = {
        "invalidate": attrs.string(),
        "script": attrs.source(),
        "use_content_based_path": attrs.bool(default = read_config("test", "use_content_based_path", "") in ["true", "True"]),
    },
)
