# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _impl(ctx):
    command = ["fbpython", "-c", "import sys; sys.exit(0)"]
    return [
        DefaultInfo(default_output = ctx.actions.write("out", "ok")),
        RunInfo(args = command),
        ExternalRunnerTestInfo(type = "custom", command = command),
    ]

simple_test = rule(impl = _impl, attrs = {})
