# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

G = exec_group()

def _impl(ctx):
    out = ctx.actions.declare_output("out")
    ctx.actions.run(["true", out.as_output()], category = "compile", identifier = "main", exec_group = G)
    return [DefaultInfo(default_output = out)]

# No platform is both macOS and remote, so `G` has no compatible execution platform.
r = rule(
    impl = _impl,
    attrs = {
        "g_constraints": attrs.exec_compatible_with(
            exec_group = G,
            default = ["//config:macos", "//config:remote"],
        ),
    },
    exec_groups = [G],
)
