# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _impl(ctx):
    out = ctx.actions.declare_output("out.txt", has_content_based_path = False)

    def body(ctx, _dynamic_artifacts, outputs):
        ctx.actions.write(outputs[out].as_output(), "42")

    ctx.actions.dynamic_output(dynamic = [], inputs = [], outputs = [out.as_output()], f = body)
    return [DefaultInfo(default_output = out)]

test_rule = rule(
    impl = _impl,
    attrs = {},
)
