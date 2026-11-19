# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _slow(ctx):
    slow = ctx.actions.declare_output("slow", has_content_based_path = False)

    ctx.actions.run(
        ["fbpython", "-c", "import time, sys; time.sleep(10); sys.exit(1)", slow.as_output()],
        category = "slow_default_output",
    )

    return [DefaultInfo(slow)]

def _mixed(ctx):
    fast = ctx.actions.declare_output("fast", has_content_based_path = False)
    slow = ctx.actions.declare_output("slow", has_content_based_path = False)

    ctx.actions.run(
        ["fbpython", "-c", "import sys; sys.exit(1)", fast.as_output()],
        category = "fast_default_output",
    )

    ctx.actions.run(
        ["fbpython", "-c", "import time, sys; time.sleep(10); sys.exit(1)", slow.as_output()],
        category = "slow_other_output",
    )

    return [DefaultInfo(default_output = fast, other_outputs = [slow])]

slow = rule(impl = _slow, attrs = {})
mixed = rule(impl = _mixed, attrs = {})
