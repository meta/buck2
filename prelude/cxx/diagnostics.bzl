# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//utils:expect.bzl", "expect")
load(":cxx_context.bzl", "get_cxx_toolchain_info")

def check_sub_target(
    ctx: AnalysisContext,
    diagnostics: dict[str, Artifact],
    error_handler: [typing.Callable, None] = None,
    output_name: str = "diagnostics.txt",
    extra_sub_targets: dict[str, Artifact] = {},
) -> (list[Provider], Artifact):
    expect(len(diagnostics) > 0)

    if len(diagnostics) == 1:
        all_diagnostics = diagnostics.values()[0]
    else:
        toolchain = get_cxx_toolchain_info(ctx)
        concatenate_diagnostics_tool = toolchain.internal_tools.concatenate_diagnostics
        all_diagnostics = ctx.actions.declare_output(output_name, has_content_based_path = False)
        ctx.actions.run(
            [
                concatenate_diagnostics_tool,
                cmd_args(all_diagnostics.as_output(), format = "--out={}"),
                diagnostics.values(),
            ],
            category = "diagnostics",
            identifier = output_name,
            error_handler = error_handler,
        )

    all_sub_targets = {short_path: [DefaultInfo(default_output = diag)] for short_path, diag in diagnostics.items()}
    for short_path, diag in extra_sub_targets.items():
        all_sub_targets[short_path] = [DefaultInfo(default_output = diag)]

    return [
        DefaultInfo(
            default_output = all_diagnostics,
            sub_targets = all_sub_targets,
        )
    ], all_diagnostics
