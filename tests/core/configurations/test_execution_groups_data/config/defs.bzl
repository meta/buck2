# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _constraint_setting(ctx):
    return [DefaultInfo(), ConstraintSettingInfo(label = ctx.label.raw_target())]

constraint_setting = rule(
    impl = _constraint_setting,
    attrs = {},
)

def _constraint_value(ctx):
    constraint_value = ConstraintValueInfo(
        setting = ctx.attrs.setting[ConstraintSettingInfo],
        label = ctx.label.raw_target(),
    )
    return [
        DefaultInfo(),
        constraint_value,
        ConfigurationInfo(
            constraints = {
                constraint_value.setting.label: constraint_value,
            },
            values = {},
        ),
    ]

constraint_value = rule(
    impl = _constraint_value,
    attrs = {"setting": attrs.dep(providers = [ConstraintSettingInfo])},
)

def _platform(ctx):
    constraints = {}
    values = {}
    for x in ctx.attrs.configuration:
        constraints |= x[ConfigurationInfo].constraints
        values |= x[ConfigurationInfo].values
    cfg = ConfigurationInfo(constraints = constraints, values = values)

    platform = ExecutionPlatformInfo(
        label = ctx.label.raw_target(),
        configuration = cfg,
        executor_config = CommandExecutorConfig(
            local_enabled = False,
            remote_enabled = True,
            remote_cache_enabled = True,
            remote_execution_properties = {
                "platform": "linux-remote-execution",
            },
            remote_execution_use_case = "buck2-testing",
        )
        if ctx.attrs.remote
        else CommandExecutorConfig(
            local_enabled = True,
            remote_enabled = False,
        ),
    )

    return [DefaultInfo(), platform, PlatformInfo(label = str(ctx.label.raw_target()), configuration = cfg)]

platform = rule(
    impl = _platform,
    attrs = {
        "configuration": attrs.list(attrs.dep(providers = [ConfigurationInfo])),
        "remote": attrs.bool(default = False),
    },
)

def _platforms(ctx):
    return [
        DefaultInfo(),
        ExecutionPlatformRegistrationInfo(
            platforms = [x[ExecutionPlatformInfo] for x in ctx.attrs.platforms],
            fallback = ctx.attrs.fallback,
        ),
    ]

platforms = rule(
    impl = _platforms,
    attrs = {
        "fallback": attrs.option(attrs.string(), default = None),
        "platforms": attrs.list(attrs.dep(providers = [ExecutionPlatformInfo])),
    },
)
