# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _action_fail(ctx):
    out1 = ctx.actions.declare_output("failed_action.json", has_content_based_path = ctx.attrs.use_content_based_path)
    out2 = ctx.actions.declare_output("failed_action.txt", has_content_based_path = ctx.attrs.use_content_based_path)

    run = ctx.actions.write(
        "run.py",
        [
            "import sys",
            "with open(sys.argv[1], 'w') as f:",
            "  f.write('json')",
            "with open(sys.argv[2], 'w') as f:",
            "  f.write('txt')",
            "sys.exit(1)",
        ],
        has_content_based_path = False,
    )

    ctx.actions.run(
        cmd_args(
            "fbpython",
            run,
            out1.as_output(),
            out2.as_output(),
        ),
        category = "test",
        outputs_for_error_handler = [out1.as_output()],
    )
    return [DefaultInfo(default_outputs = [out1, out2])]

action_fail = rule(
    impl = _action_fail,
    attrs = {
        "use_content_based_path": attrs.bool(default = read_config("test", "use_content_based_path", "") in ["true", "True"]),
    },
)

def _undeclared_output(ctx):
    declared = ctx.actions.declare_output("failed_action.json", has_content_based_path = ctx.attrs.use_content_based_path)
    undeclared = ctx.actions.declare_output("failed_action.txt", has_content_based_path = ctx.attrs.use_content_based_path)
    ctx.actions.run(
        cmd_args(
            "fbpython",
            "-c",
            "import sys; sys.exit(1)",
            declared.as_output(),
        ),
        category = "test",
        outputs_for_error_handler = [undeclared.as_output()],
    )
    return [DefaultInfo(default_outputs = [declared])]

undeclared_output = rule(
    impl = _undeclared_output,
    attrs = {
        "use_content_based_path": attrs.bool(default = read_config("test", "use_content_based_path", "") in ["true", "True"]),
    },
)
