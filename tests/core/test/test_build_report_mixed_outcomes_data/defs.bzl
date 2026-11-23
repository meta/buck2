# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _fail_build(ctx):
    output = ctx.actions.declare_output("never_written")
    ctx.actions.run(
        cmd_args(["fbpython", "-c", "raise SystemExit(1)"], hidden = output.as_output()),
        category = "fail_build",
        local_only = True,
    )
    return [DefaultInfo(default_output = output)]

fail_build = rule(impl = _fail_build, attrs = {})

def _pass_test(_ctx):
    return [
        DefaultInfo(),
        ExternalRunnerTestInfo(
            command = ["fbpython", "-c", "raise SystemExit(0)"],
            type = "custom",
        ),
    ]

pass_test = rule(impl = _pass_test, attrs = {})
