# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _simple(ctx):
    output = ctx.actions.declare_output("output", has_content_based_path = False)
    run = ctx.actions.write(
        "run.py",
        [
            "import sys",
            "with open(sys.argv[1], 'r') as f:",
            "  content = f.read()",
            "with open(sys.argv[2], 'w') as f:",
            "  f.write(content)",
        ],
        has_content_based_path = False,
    )
    ctx.actions.run(
        cmd_args(["fbpython", run, ctx.attrs.input, output.as_output()]),
        category = "test_category",
    )

    return [DefaultInfo(default_output = output)]

simple = rule(impl = _simple, attrs = {"input": attrs.source()})
