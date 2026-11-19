# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _impl_cp(ctx):
    out = ctx.actions.declare_output("out", has_content_based_path = False)
    ctx.actions.run(
        ["cp", ctx.attrs.src, out.as_output()],
        category = "cp",
        local_only = ctx.attrs.local_only,
        env = {"CACHE_BUSTER": str(ctx.attrs.local_only)},
    )
    return [DefaultInfo(out)]

cp = rule(
    attrs = {
        "local_only": attrs.bool(default = False),
        "src": attrs.source(),
    },
    impl = _impl_cp,
)
