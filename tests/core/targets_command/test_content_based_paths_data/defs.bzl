# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _write_with_content_based_path_impl(ctx):
    out = ctx.actions.declare_output("out.txt", has_content_based_path = True)
    return [DefaultInfo(default_output = ctx.actions.write(out, "out"))]

write_with_content_based_path = rule(
    impl = _write_with_content_based_path_impl,
    attrs = {},
)
