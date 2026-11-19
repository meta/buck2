# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _config_setting(ctx):
    return [DefaultInfo(), ConstraintSettingInfo(label = ctx.label.raw_target())]

config_setting = rule(
    impl = _config_setting,
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
        # Also expose a ConfigurationInfo so the target can be used directly as a select() key.
        ConfigurationInfo(
            constraints = {
                constraint_value.setting.label: constraint_value,
            },
            values = {},
        ),
    ]

constraint_value = rule(
    impl = _constraint_value,
    attrs = {
        "setting": attrs.dep(providers = [ConstraintSettingInfo]),
    },
)

def _target_platform(ctx):
    constraint_value = ctx.attrs.constraint_value[ConstraintValueInfo]
    configuration = ConfigurationInfo(
        constraints = {
            constraint_value.setting.label: constraint_value,
        },
        values = {},
    )
    return [
        DefaultInfo(),
        PlatformInfo(
            label = str(ctx.label.raw_target()),
            configuration = configuration,
        ),
    ]

target_platform = rule(
    impl = _target_platform,
    attrs = {
        "constraint_value": attrs.dep(providers = [ConstraintValueInfo]),
    },
)
