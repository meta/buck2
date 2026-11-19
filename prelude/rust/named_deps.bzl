# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//utils:argfile.bzl", "at_argfile")
load("@prelude//utils:type_defs.bzl", "is_list")
load(":context.bzl", "CompileContext")

# Write a file containing all the dynamically-generated dependency names. This
# isn't used in the course of any Buck builds, but is needed by rust-project to
# supply an accurate dependency graph to rust-analyzer..
def write_named_deps_names(ctx: AnalysisContext, compile_ctx: CompileContext) -> Artifact | None:
    if not is_list(ctx.attrs.named_deps):
        return None

    use_cbp = getattr(ctx.attrs, "use_content_based_paths", False)
    named_deps_names = ctx.actions.declare_output("named_deps", has_content_based_path = use_cbp)
    ctx.actions.run(
        cmd_args(
            compile_ctx.internal_tools_info.rustc_action,
            cmd_args(named_deps_names.as_output(), format = "--echo={}"),
            at_argfile(
                actions = ctx.actions,
                name = "named_deps.args",
                args = [name for name, _dep in ctx.attrs.named_deps],
                allow_args = True,
                has_content_based_path = use_cbp,
            ),
        ),
        category = "named_deps",
    )
    return named_deps_names
