# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _artifacts(ctx):
    fast = ctx.actions.write("fast", "", has_content_based_path = False)
    slow = ctx.actions.declare_output("slow", has_content_based_path = False)

    ctx.actions.run(
        [
            "fbpython",
            "-c",
            "import time, sys; time.sleep(5); open(sys.argv[1], 'w')",
            slow.as_output(),
        ],
        category = "slow",
    )

    return [DefaultInfo(slow, other_outputs = [fast])]

artifacts = rule(impl = _artifacts, attrs = {})
