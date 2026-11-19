# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _run_action_impl(ctx):
    out = ctx.actions.declare_output("out.txt", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(
            "python3",
            "-c",
            cmd_args(
                "import sys; open(sys.argv[1], 'w').write('hello')",
                out.as_output(),
            ),
        ),
        category = "write_file",
    )
    return [DefaultInfo(default_output = out)]

run_action = rule(impl = _run_action_impl, attrs = {})
