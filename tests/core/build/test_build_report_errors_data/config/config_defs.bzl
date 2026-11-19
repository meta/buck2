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
        # Provide `ConfigurationInfo` from `constraint_value` so it could be used as select key.
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
    cfg = ConfigurationInfo(constraints = ctx.attrs.configuration[ConfigurationInfo].constraints, values = {})

    return [DefaultInfo(), PlatformInfo(label = str(ctx.label.raw_target()), configuration = cfg)]

platform = rule(
    impl = _platform,
    attrs = {"configuration": attrs.dep(providers = [ConfigurationInfo])},
)
