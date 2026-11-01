# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _write_impl(ctx: AnalysisContext) -> list[Provider]:
    out = ctx.actions.declare_output("payload.txt", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(["sh", "-c", 'printf payload > "$1"', "--", out.as_output()]),
        category = "write_payload",
        local_only = True,
    )
    return [DefaultInfo(default_output = out)]

write_payload = rule(impl = _write_impl, attrs = {})

def _consume_impl(ctx: AnalysisContext) -> list[Provider]:
    out = ctx.actions.declare_output("consumed.txt", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(["sh", "-c", 'cat "$1" > "$2"', "--", ctx.attrs.src, out.as_output()]),
        category = "consume_payload",
        local_only = True,
    )
    return [DefaultInfo(default_output = out)]

consume_payload = rule(impl = _consume_impl, attrs = {"src": attrs.source()})
