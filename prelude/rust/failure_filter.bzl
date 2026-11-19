# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(":context.bzl", "CompileContext")

# This creates an action which takes a buildstatus json artifact as an input, and a list of other
# artifacts. If all those artifacts are present in the buildstatus as successfully generated, then
# the action will succeed with those artifacts as outputs. Otherwise it fails.
# Either way it streams whatever stderr content there is to stream.
def failure_filter(
    ctx: AnalysisContext,
    compile_ctx: CompileContext,
    predeclared_output: Artifact | None,
    build_status: Artifact,
    required: Artifact,
    stderr: Artifact,
    identifier: str,
) -> Artifact:
    failure_filter_action = compile_ctx.internal_tools_info.failure_filter_action

    if predeclared_output:
        output = predeclared_output
    else:
        output = ctx.actions.declare_output("out/" + required.short_path, has_content_based_path = getattr(ctx.attrs, "use_content_based_paths", False))

    cmd = cmd_args(
        failure_filter_action,
        "--stderr",
        stderr,
        "--required-file",
        required.short_path,
        required,
        output.as_output(),
        "--build-status",
        build_status,
    )

    toolchain_info = compile_ctx.toolchain_info
    ctx.actions.run(
        cmd,
        category = "failure_filter",
        identifier = identifier,
        error_handler = toolchain_info.rust_error_handler,
    )

    return output
