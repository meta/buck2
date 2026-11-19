# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _impl(ctx):
    out = ctx.actions.declare_output("slow", has_content_based_path = False)
    ctx.actions.run(
        ["fbpython", "-c", "import time, sys; time.sleep(30); open(sys.argv[1], 'w').close()", out.as_output()],
        category = "slow",
    )
    return [DefaultInfo(default_output = out)]

slow = rule(
    impl = _impl,
    attrs = {},
)
