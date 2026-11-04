# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _write_impl(ctx):
    out = ctx.actions.write("out.txt", ctx.attrs.content, has_content_based_path = False)
    return [DefaultInfo(default_output = out)]

write = rule(
    impl = _write_impl,
    attrs = {
        "content": attrs.string(),
    },
)

def _consume_impl(ctx):
    out = ctx.actions.declare_output("copy.txt", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(["cp", ctx.attrs.dep[DefaultInfo].default_outputs[0], out.as_output()]),
        category = "consume",
    )
    return [DefaultInfo(default_output = out)]

# Reads `dep`'s output in an action, which needs that output on disk when the action runs here
# and in the CAS when it runs remotely.
consume = rule(
    impl = _consume_impl,
    attrs = {
        "dep": attrs.dep(),
    },
)
