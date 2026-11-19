# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _impl(ctx):
    tmp = ctx.actions.write("temp.txt", "".join([s + "\n" for s in ctx.attrs.data]), has_content_based_path = False)
    out = ctx.actions.declare_output("out.txt", has_content_based_path = False)

    # Good enough for tests
    if host_info().os.is_windows:
        files = cmd_args(tmp, ctx.attrs.srcs, delimiter = "+")
        cmd = cmd_args("cmd", "/c", "copy", "/b", files, out.as_output())
        ctx.actions.run(cmd, category = "run")
    else:
        cmd = cmd_args("cat", tmp, ctx.attrs.srcs, ">", out.as_output(), delimiter = " ")
        ctx.actions.run(cmd_args("bash", "-c", cmd), category = "run")
    return [DefaultInfo(default_output = out)]

test_rule = rule(
    impl = _impl,
    attrs = {
        "data": attrs.list(attrs.string()),
        "srcs": attrs.list(attrs.source()),
    },
)

def _export_impl(ctx):
    return [DefaultInfo(default_output = ctx.attrs.src)]

export_file = rule(
    impl = _export_impl,
    attrs = {
        "src": attrs.source(),
    },
)
