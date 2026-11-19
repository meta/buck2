# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _dog_and_bone(ctx):
    dir = ctx.actions.declare_output("dog", has_content_based_path = False)
    ctx.actions.run(["touch", dir.as_output()], category = "mkdir")

    out = ctx.actions.declare_output("bone", has_content_based_path = False)
    ctx.actions.run(
        ["ln", "-s", cmd_args(dir, relative_to = (out, 1)), out.as_output()],
        category = "symlink",
    )

    return [DefaultInfo(default_output = out)]

dog_and_bone = rule(
    impl = _dog_and_bone,
    attrs = {},
)
