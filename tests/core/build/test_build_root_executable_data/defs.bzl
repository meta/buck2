# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _top(ctx):
    out = ctx.actions.declare_output("out", has_content_based_path = False)

    ctx.actions.run(
        [ctx.attrs.src, out.as_output()],
        category = "test",
    )

    return [DefaultInfo(out)]

top = rule(impl = _top, attrs = {"src": attrs.source()})
