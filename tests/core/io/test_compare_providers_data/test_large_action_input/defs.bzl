# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _dep_impl(ctx):
    out = ctx.actions.declare_output("dep", has_content_based_path = False)
    ctx.actions.run(
        [
            "fbpython",
            ctx.attrs.script,
            out.as_output(),
        ],
        category = "test",
    )
    return [DefaultInfo(default_output = out)]

dep = rule(
    impl = _dep_impl,
    attrs = {
        "script": attrs.source(),
    },
)

def _large_action_input(ctx):
    dep = ctx.attrs.dep[DefaultInfo].default_outputs[0]
    out = ctx.actions.declare_output("out", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(
            [
                "fbpython",
                "-c",
                "import sys, os; file_size = os.path.getsize(sys.argv[2]); open(sys.argv[1], 'w').write(str(file_size))",
            ],
            out.as_output(),
            dep,
        ),
        category = "test",
    )

    return [DefaultInfo(out)]

large_action_input = rule(
    impl = _large_action_input,
    attrs = {
        "dep": attrs.dep(),
    },
)
