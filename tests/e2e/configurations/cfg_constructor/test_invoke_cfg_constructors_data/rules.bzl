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
    is_configuration_rule = True,
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
    is_configuration_rule = True,
    attrs = {"setting": attrs.dep(providers = [ConstraintSettingInfo])},
)

def _test_platform_impl(ctx):
    _unused = ctx  # buildifier: disable=unused-variable
    return [
        DefaultInfo(),
        PlatformInfo(
            label = str(ctx.label.raw_target()),
            configuration = ConfigurationInfo(
                constraints = {},
                values = {},
            ),
        ),
    ]

test_platform = rule(
    impl = _test_platform_impl,
    attrs = {},
)

def _test_rule(ctx):
    _unused = ctx  # buildifier: disable=unused-variable
    return [
        DefaultInfo(),
    ]

test_rule = rule(
    impl = _test_rule,
    attrs = {},
)
