# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _simple_impl(ctx):
    out = ctx.actions.declare_output("out", has_content_based_path = False)
    ctx.actions.write(out, ctx.attrs.arg_attr)

    return [DefaultInfo(default_output = out)]

simple = rule(
    impl = _simple_impl,
    attrs = {
        "arg_attr": attrs.list(attrs.arg(anon_target_compatible = True), default = []),
    },
)
