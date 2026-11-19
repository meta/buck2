# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _fail(ctx):
    out = ctx.actions.declare_output("out", has_content_based_path = False)
    ctx.actions.run(cmd_args("false", hidden = out.as_output()), category = "fail")
    return [DefaultInfo(out)]

fail = rule(attrs = {}, impl = _fail)

def _one(ctx):
    return [DefaultInfo(default_output = ctx.actions.write("out", "one", has_content_based_path = False))]

one = rule(
    impl = _one,
    attrs = {},
)
