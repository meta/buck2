# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//cfg/exec_platform:marker.bzl", "get_exec_platform_marker")

def _execution_platform(ctx):
    return [
        DefaultInfo(),
        ExecutionPlatformInfo(
            label = ctx.label.raw_target(),
            configuration = ctx.attrs.platform[PlatformInfo].configuration,
            executor_config = CommandExecutorConfig(
                local_enabled = True,
                remote_enabled = False,
            ),
        ),
    ]

execution_platform = rule(
    impl = _execution_platform,
    attrs = {
        "platform": attrs.dep(providers = [PlatformInfo]),
    },
)

def _execution_platforms(ctx):
    platforms = (
        [p[ExecutionPlatformInfo] for p in ctx.attrs.platforms]
        if ctx.attrs.platforms
        else [
            ExecutionPlatformInfo(
                label = ctx.label.raw_target(),
                configuration = ConfigurationInfo(constraints = {}, values = {}),
                executor_config = CommandExecutorConfig(local_enabled = True, remote_enabled = False),
            )
        ]
    )

    return [
        DefaultInfo(),
        ExecutionPlatformRegistrationInfo(
            platforms = platforms,
            exec_marker_constraint = get_exec_platform_marker(),
        ),
    ]

execution_platforms = rule(
    impl = _execution_platforms,
    attrs = {
        "platforms": attrs.option(attrs.list(attrs.dep(providers = [ExecutionPlatformInfo])), default = None),
    },
)
