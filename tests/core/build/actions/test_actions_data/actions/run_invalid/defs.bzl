# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _artifact_cycle(ctx):
    out = ctx.actions.declare_output("out", has_content_based_path = False)

    # Cycle
    cmd = cmd_args(out.as_output())
    cmd.add(cmd_args(hidden = cmd))
    ctx.actions.run(cmd, category = "test")

    return DefaultInfo(out)

artifact_cycle = rule(impl = _artifact_cycle, attrs = {})
