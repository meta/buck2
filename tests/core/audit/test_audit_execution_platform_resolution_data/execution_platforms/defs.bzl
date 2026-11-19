# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# @nolint

def _execution_platform(ctx):
    return [
        DefaultInfo(),
        ExecutionPlatformInfo(
            label = ctx.label.raw_target(),
            configuration = ctx.attrs.os_configuration[ConfigurationInfo],
            executor_config = CommandExecutorConfig(
                local_enabled = True,
                remote_enabled = False,
            ),
        ),
    ]

execution_platform = rule(
    impl = _execution_platform,
    attrs = {
        "os_configuration": attrs.dep(providers = [ConfigurationInfo]),
    },
)

def _execution_platforms(ctx):
    return [
        DefaultInfo(),
        ExecutionPlatformRegistrationInfo(
            platforms = [p[ExecutionPlatformInfo] for p in ctx.attrs.platforms],
            fallback = ctx.attrs.fallback,
        ),
    ]

execution_platforms = rule(
    impl = _execution_platforms,
    attrs = {
        "fallback": attrs.option(attrs.string(), default = None),
        "platforms": attrs.list(attrs.dep(providers = [ExecutionPlatformInfo])),
    },
)
