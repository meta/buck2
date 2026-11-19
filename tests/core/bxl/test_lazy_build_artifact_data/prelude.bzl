# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _duplicate_output_impl(ctx):
    out = ctx.actions.declare_output("bar.txt", has_content_based_path = False)
    data = ctx.actions.write("foo.txt", "42", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(["cp", data, out.as_output()]),
        category = "copy",
    )
    return [DefaultInfo(default_outputs = [data, out])]

duplicate_output = rule(
    impl = _duplicate_output_impl,
    attrs = {},
)

def _fail_duplicate(ctx):
    out = ctx.actions.declare_output("bar.txt", has_content_based_path = False)
    data = ctx.actions.write("foo.txt", "42", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(["wrong_cp", data, out.as_output()]),
        category = "copy",
    )
    return [DefaultInfo(default_outputs = [data, out])]

fail_duplicate = rule(
    impl = _fail_duplicate,
    attrs = {},
)
