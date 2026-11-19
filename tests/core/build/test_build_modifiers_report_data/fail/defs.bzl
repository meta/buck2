# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _make_failing_action(ctx):
    out = ctx.actions.declare_output("fail", has_content_based_path = False)
    ctx.actions.run(cmd_args("false", hidden = out.as_output()), category = "fail")
    return out

def _fail_impl(ctx):
    return [DefaultInfo(default_outputs = [_make_failing_action(ctx)])]

fail = rule(
    impl = _fail_impl,
    attrs = {},
)
