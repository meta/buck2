# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

cache_buster = read_config("test", "cache_buster", "")

def _top(ctx):
    output = ctx.actions.declare_output("output", has_content_based_path = False)
    run = ctx.actions.write(
        "run.py",
        [
            "import os",
            "import sys",
            'build_id = os.environ["BUCK_BUILD_ID"]',
            "with open(sys.argv[1], 'w') as f:",
            "  f.write(f'{build_id}\\n')",
        ],
        has_content_based_path = False,
    )
    ctx.actions.run(
        cmd_args(["fbpython", run, output.as_output()]),
        category = "test_category",
        env = {
            "cache_buster": cache_buster,
        },
    )

    return [DefaultInfo(default_output = output)]

top = rule(impl = _top, attrs = {})
