# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _slow_impl(ctx: AnalysisContext) -> list[Provider]:
    outs = {}
    for i in range(ctx.attrs.count):
        o = ctx.actions.declare_output("out/{}".format(i), has_content_based_path = False)
        ctx.actions.run(
            ["fbpython", ctx.attrs.src, ctx.attrs.duration, ctx.attrs.pids, o.as_output()],
            category = "test",
            identifier = str(i),
        )
        outs[str(i)] = o

    out = ctx.actions.symlinked_dir("outs", outs, has_content_based_path = False)
    return [DefaultInfo(out)]

slow_actions = rule(
    impl = _slow_impl,
    attrs = {
        "count": attrs.int(),
        "duration": attrs.string(),
        "pids": attrs.string(),
        "src": attrs.source(),
    },
)
