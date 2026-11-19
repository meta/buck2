# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _buildable_impl(ctx):
    out = ctx.actions.write(ctx.attrs.out, ctx.attrs.content, has_content_based_path = False)
    return [DefaultInfo(default_output = out)]

foo_buildable = rule(
    impl = _buildable_impl,
    attrs = {
        "content": attrs.string(default = ""),
        "out": attrs.string(),
        "srcs": attrs.list(attrs.source()),
    },
)
