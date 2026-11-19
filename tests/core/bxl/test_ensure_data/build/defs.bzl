# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _error_handler(ctx: ActionErrorCtx) -> list[ActionSubError]:
    err = ctx.new_sub_error(
        category = "fail_build_error",
        message = "fail to build",
        file = "foo.cpp",
        lnum = 42,
        col = 2,
    )
    return [err]

def _impl(ctx):
    out = ctx.actions.declare_output("out.txt", has_content_based_path = False)
    ctx.actions.run(
        cmd_args("exit", "1", hidden = out.as_output()),
        category = "run",
        error_handler = _error_handler,
    )
    return [DefaultInfo(default_output = out)]

fail_build = rule(
    impl = _impl,
    attrs = {},
)
