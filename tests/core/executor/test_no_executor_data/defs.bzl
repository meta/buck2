# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _run_action(ctx):
    out = ctx.actions.declare_output("out", has_content_based_path = False)
    ctx.actions.run(cmd_args(out.as_output()), category = "test")
    return [DefaultInfo(out)]

run_action = rule(
    impl = _run_action,
    attrs = {},
)

def _execution_platforms(ctx):
    return [
        DefaultInfo(),
        ExecutionPlatformRegistrationInfo(
            platforms = [
                ExecutionPlatformInfo(
                    label = ctx.label.raw_target(),
                    configuration = ConfigurationInfo(constraints = {}, values = {}),
                    executor_config = CommandExecutorConfig(local_enabled = False, remote_cache_enabled = False, remote_enabled = False),
                )
            ]
        ),
    ]

execution_platforms = rule(
    impl = _execution_platforms,
    attrs = {},
)
