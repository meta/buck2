# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _impl_cp(ctx):
    out = ctx.actions.declare_output("out", has_content_based_path = False)
    ctx.actions.run(["cp", ctx.attrs.src, out.as_output()], category = "cp")
    return [DefaultInfo(out)]

cp = rule(attrs = {"src": attrs.source()}, impl = _impl_cp)

def _impl_exec_platforms(ctx):
    configuration = ConfigurationInfo(constraints = {}, values = {})

    platform = ExecutionPlatformInfo(
        label = ctx.label.raw_target(),
        configuration = configuration,
        executor_config = CommandExecutorConfig(
            local_enabled = False,
            remote_enabled = True,
            remote_execution_properties = {
                "platform": "linux-remote-execution",
            },
            remote_execution_use_case = "buck2-testing",
        ),
    )

    return [
        DefaultInfo(),
        configuration,
        platform,
        ExecutionPlatformRegistrationInfo(platforms = [platform]),
    ]

exec_platforms = rule(attrs = {}, impl = _impl_exec_platforms)
