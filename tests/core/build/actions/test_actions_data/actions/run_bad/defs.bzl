# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _run_invalid_command_impl(ctx: AnalysisContext) -> list[Provider]:
    out = ctx.actions.declare_output("out", has_content_based_path = False)
    ctx.actions.run(
        ["this_binary_does_not_exist", out.as_output()],
        category = "test",
        local_only = ctx.attrs.local_only,
    )
    return [DefaultInfo(default_output = out)]

run_invalid_command = rule(impl = _run_invalid_command_impl, attrs = {"local_only": attrs.bool()})

def _run_odd_exit_code_impl(ctx: AnalysisContext) -> list[Provider]:
    out = ctx.actions.declare_output("out", has_content_based_path = False)
    ctx.actions.run(
        cmd_args("fbpython", "-c", "import sys; import time; time.sleep(1); sys.exit(int(sys.argv[1]))", ctx.attrs.exit_code, out.as_output()),
        category = "test",
    )
    return [DefaultInfo(default_output = out)]

run_odd_exit_code = rule(impl = _run_odd_exit_code_impl, attrs = {"exit_code": attrs.string(default = "1")})

def _run_symlink_with_backslash_impl(ctx: AnalysisContext) -> list[Provider]:
    out = ctx.actions.declare_output("out", has_content_based_path = False)
    ctx.actions.run(
        cmd_args("fbpython", "-c", "import os, sys; os.symlink(r'a\\b', sys.argv[1])", out.as_output()),
        category = "test",
        local_only = True,
    )
    return [DefaultInfo(default_output = out)]

run_symlink_with_backslash = rule(impl = _run_symlink_with_backslash_impl, attrs = {})
