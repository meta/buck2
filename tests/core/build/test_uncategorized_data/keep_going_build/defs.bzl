# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _top(ctx):
    fast = ctx.actions.declare_output("fast", has_content_based_path = False)
    slow = ctx.actions.declare_output("slow", has_content_based_path = False)
    out = ctx.actions.declare_output("out", has_content_based_path = False)

    ctx.actions.run(
        ["fbpython", "-c", "import sys; sys.exit(1)", fast.as_output()],
        category = "fast_action",
    )

    ctx.actions.run(
        ["fbpython", "-c", "import time, sys; time.sleep(10); sys.exit(1)", slow.as_output()],
        category = "slow_action",
    )

    # Won't actually run because the others will fail
    ctx.actions.run(
        ["bugbugbug", fast, slow, out.as_output()],
        category = "noop",
    )

    return [DefaultInfo(default_output = out)]

top = rule(impl = _top, attrs = {})
