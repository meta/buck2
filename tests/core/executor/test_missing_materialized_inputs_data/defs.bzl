# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _test_binary_impl(ctx):
    executable = ctx.actions.write(
        "test.sh",
        "#!/bin/sh\nexit 0\n",
        is_executable = True,
        has_content_based_path = False,
    )
    return [
        DefaultInfo(default_output = executable),
        ExternalRunnerTestInfo(
            command = [executable],
            type = "custom",
        ),
    ]

test_binary = rule(impl = _test_binary_impl, attrs = {})

def _failing_action_impl(ctx):
    out = ctx.actions.declare_output("out", has_content_based_path = False)
    ctx.actions.run(
        [ctx.attrs.command, out.as_output()],
        category = "failing_action",
        identifier = ctx.attrs.name,
        local_only = True,
    )
    return [DefaultInfo(default_output = out)]

failing_action = rule(
    impl = _failing_action_impl,
    attrs = {"command": attrs.string()},
)
