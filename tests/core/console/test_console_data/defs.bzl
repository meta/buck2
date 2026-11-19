# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _slow_genrule_impl(ctx):
    out = ctx.actions.declare_output("out.txt", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(["sh", "-c", 'sleep 10 && echo done > "$1"', "--", out.as_output()]),
        category = "run",
        local_only = True,
    )
    return [DefaultInfo(default_outputs = [out])]

slow_genrule = rule(impl = _slow_genrule_impl, attrs = {})
