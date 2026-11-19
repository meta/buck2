# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _run_with_timeout_impl(ctx):
    output = ctx.actions.declare_output(ctx.attrs.out, has_content_based_path = False)
    ctx.actions.run(
        ["sh", ctx.attrs.script, output.as_output()],
        category = "test",
        local_only = True,
        timeout_seconds = ctx.attrs.timeout_seconds,
    )
    return [DefaultInfo(default_output = output)]

run_with_timeout = rule(
    impl = _run_with_timeout_impl,
    attrs = {
        "out": attrs.string(default = "out.txt"),
        "script": attrs.source(),
        "timeout_seconds": attrs.int(),
    },
)
