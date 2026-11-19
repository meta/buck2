# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _run_python(ctx):
    return [
        DefaultInfo(),
        RunInfo(args = cmd_args("fbpython", "-c", ctx.attrs.script)),
    ]

run_python = rule(
    impl = _run_python,
    attrs = {
        "script": attrs.string(),
    },
)

def _build_fail(ctx):
    out = ctx.actions.declare_output("out.txt", has_content_based_path = False)
    ctx.actions.run(
        cmd_args("exit", "1", hidden = out.as_output()),
        category = "run",
    )
    return [DefaultInfo(default_output = out), RunInfo(args = cmd_args(out))]

build_fail = rule(
    impl = _build_fail,
    attrs = {},
)
