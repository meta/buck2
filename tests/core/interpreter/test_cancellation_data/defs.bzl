# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# @nolint

def spin():
    for i in range(2147483647):
        for j in range(2147483647):
            for k in range(2147483647):
                pass

def loop_long(kind):
    if read_root_config("should", "loop", "") == kind:
        spin()

def _noop(ctx):
    outs = []
    out = ctx.actions.write("out.txt", ctx.attrs.name, has_content_based_path = False)
    outs.append(out)
    loop_long("analysis")

    return [DefaultInfo(default_outputs = outs)]

noop = rule(
    impl = _noop,
    attrs = {},
)
