# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _impl(ctx):
    out = ctx.actions.declare_output("out.txt", has_content_based_path = False)
    py = cmd_args(
        "import time; time.sleep(3); ",
        "import sys; open(sys.argv[1], 'w').write('')",
        delimiter = "",
    )

    ctx.actions.run(
        cmd_args("fbpython", "-c", py, out.as_output()),
        category = "run_python",
    )

    return [DefaultInfo(default_output = out)]

long_action = rule(
    impl = _impl,
    attrs = {},
)
