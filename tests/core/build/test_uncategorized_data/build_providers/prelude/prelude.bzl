# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _simple_test_impl(ctx):
    return [
        DefaultInfo(default_output = ctx.actions.write("build", "", has_content_based_path = False)),
        RunInfo(
            args = cmd_args([
                ctx.actions.write("run", "", has_content_based_path = False),
            ])
        ),
        ExternalRunnerTestInfo(
            type = "custom",
            command = [
                cmd_args([
                    ctx.actions.write("test", "", has_content_based_path = False),
                ])
            ],
            env = {},
            labels = [],
            contacts = [],
        ),
    ]

simple_test = rule(
    impl = _simple_test_impl,
    attrs = {},
)
