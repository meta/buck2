# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _slow_impl(ctx: AnalysisContext) -> list[Provider]:
    out = ctx.actions.declare_output("out", has_content_based_path = False)
    ctx.actions.run(
        ["fbpython", ctx.attrs.src, out.as_output()],
        category = "slow",
    )
    return [DefaultInfo(out)]

slow_actions = rule(
    impl = _slow_impl,
    attrs = {
        "src": attrs.source(),
    },
)
