# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _slow_subtarget_impl(ctx: AnalysisContext) -> list[Provider]:
    fast = ctx.actions.write("fast.txt", "", has_content_based_path = False)
    slow = ctx.actions.declare_output("slow.txt", has_content_based_path = False)
    ctx.actions.run(
        [
            "fbpython",
            "-c",
            "import sys, time; time.sleep(10); open(sys.argv[1], 'w').close()",
            slow.as_output(),
        ],
        category = "slow",
    )
    return [
        DefaultInfo(
            default_outputs = [fast],
            sub_targets = {"slow": [DefaultInfo(default_outputs = [fast, slow])]},
        )
    ]

slow_subtarget = rule(
    impl = _slow_subtarget_impl,
    attrs = {},
)
