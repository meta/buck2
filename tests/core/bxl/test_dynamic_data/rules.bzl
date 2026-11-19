# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

FooInfo = provider(fields = ["name", "env"])

def _impl_rule_with_output(ctx):
    out = ctx.actions.write(ctx.attrs.name, ctx.attrs.content, has_content_based_path = False)
    return [DefaultInfo(default_output = out)]

rule_with_output = rule(
    impl = _impl_rule_with_output,
    attrs = {
        "content": attrs.string(),
    },
)

def _impl_foo_rule(ctx):
    return [DefaultInfo(), FooInfo(name = ctx.attrs.name + "_foo", env = ctx.attrs.env.get("OUTPUT", None))]

foo_rule = rule(
    impl = _impl_foo_rule,
    attrs = {
        "env": attrs.dict(key = attrs.string(), value = attrs.arg(), sorted = False, default = {}),
    },
)

def _impl_foo_runnable(_ctx):
    return [
        DefaultInfo(),
        RunInfo([
            "fbpython",
            "-c",
            "import sys; sys.exit(0)",
        ]),
    ]

foo_runnable = rule(impl = _impl_foo_runnable, attrs = {})

# not really a toolchain rule, but just used to validate exec platform is propagated correctly
foo_toolchain = rule(impl = _impl_foo_runnable, attrs = {}, is_toolchain_rule = True)

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
