# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _my_rule_impl(ctx):
    a = ctx.actions.declare_output("a.txt", has_content_based_path = False)
    b = ctx.actions.declare_output("b.txt", has_content_based_path = False)
    ctx.actions.run(cmd_args("write_to", a.as_output()), category = "foo", identifier = "x")
    ctx.actions.run(cmd_args("write_to", b.as_output()), category = "foo", identifier = "x")
    return [
        DefaultInfo(
            default_outputs = [a, b],
        )
    ]

my_rule = rule(
    impl = _my_rule_impl,
    attrs = {},
)
