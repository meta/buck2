# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _failing_output(ctx, name):
    out = ctx.actions.declare_output(name)
    ctx.actions.run(
        cmd_args("false", out.as_output()),
        category = name,
    )
    return out

def _passing_test_info():
    return ExternalRunnerTestInfo(
        command = ["fbpython", "-c", "import sys; sys.exit(0)"],
        type = "custom",
    )

def _failing_default_info_impl(ctx):
    return [
        DefaultInfo(default_output = _failing_output(ctx, "failing_default_info")),
        _passing_test_info(),
    ]

failing_default_info_test = rule(attrs = {}, impl = _failing_default_info_impl)

def _failing_run_info_impl(ctx):
    return [
        DefaultInfo(),
        RunInfo(args = cmd_args(_failing_output(ctx, "failing_run_info"))),
        _passing_test_info(),
    ]

failing_run_info_test = rule(attrs = {}, impl = _failing_run_info_impl)
