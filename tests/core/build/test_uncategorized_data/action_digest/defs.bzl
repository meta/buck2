# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _test(ctx: AnalysisContext):
    out = ctx.actions.declare_output("out", has_content_based_path = False)

    ctx.actions.run(
        [
            "fbpython",
            ctx.attrs.script,
            out.as_output(),
        ],
        category = "check",
        identifier = "out",
    )

    return [DefaultInfo(out)]

test = rule(attrs = {"script": attrs.source()}, impl = _test)
