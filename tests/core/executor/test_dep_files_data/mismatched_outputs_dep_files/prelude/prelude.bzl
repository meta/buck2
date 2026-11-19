# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _test_impl(ctx):
    out = ctx.actions.declare_output(ctx.attrs.prefix, dir = True, has_content_based_path = False)

    ctx.actions.run(
        [
            "mkdir",
            "-p",
            cmd_args(out.as_output(), format = "{{}}/{}".format(ctx.attrs.suffix)),
        ],
        category = "test",
    )

    return [DefaultInfo(out)]

test = rule(
    attrs = {
        "prefix": attrs.string(default = ""),
        "suffix": attrs.string(default = ""),
    },
    impl = _test_impl,
)
