# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

# @nolint

def _impl(ctx: AnalysisContext) -> list[Provider]:
    # To trigger the bug we need to specify both `prefix` and `filename` arguments.
    dir = ctx.actions.declare_output("one", "two", has_content_based_path = False)

    def _dyn(ctx, artifacts, outputs, dir = dir):
        # The bug is here: artifacts are not equal, and map lookup fails.
        dir = outputs[dir]
        ctx.actions.write(dir, "x")

    ctx.actions.dynamic_output(
        dynamic = [],
        inputs = [],
        outputs = [dir.as_output()],
        f = _dyn,
    )

    return [DefaultInfo(default_output = dir)]

bug = rule(
    impl = _impl,
    attrs = {},
)
