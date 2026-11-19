# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _graph_test_impl(ctx):
    data = ctx.actions.write("data.txt", "data")
    return [
        DefaultInfo(),
        ExternalRunnerTestInfo(
            command = ["fbpython", "-c", "import sys; sys.exit({})".format(ctx.attrs.exit_code), data],
            type = "custom",
        ),
    ]

graph_test = rule(attrs = {"exit_code": attrs.int()}, impl = _graph_test_impl)
