# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _test(ctx: AnalysisContext):
    local = ctx.actions.declare_output("local", has_content_based_path = False)
    remote = ctx.actions.declare_output("remote", has_content_based_path = False)

    ctx.actions.run(
        [
            "fbpython",
            ctx.attrs.script,
            local.as_output(),
            "local",
        ],
        category = "check",
        identifier = "local",
        prefer_local = True,
    )

    ctx.actions.run(
        [
            "fbpython",
            ctx.attrs.script,
            remote.as_output(),
            "remote",
        ],
        category = "check",
        identifier = "remote",
        prefer_remote = True,
    )

    return [DefaultInfo(other_outputs = [local, remote])]

test = rule(attrs = {"script": attrs.source()}, impl = _test)
