# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//transitions:constraint_overrides.bzl", "constraint_overrides")

def _setting_impl(ctx):
    return [DefaultInfo(), ConstraintSettingInfo(label = ctx.label.raw_target())]

setting = rule(impl = _setting_impl, attrs = {}, is_configuration_rule = True)

def _value_impl(ctx):
    value = ConstraintValueInfo(setting = ctx.attrs.setting[ConstraintSettingInfo], label = ctx.label.raw_target())
    return [DefaultInfo(), value, ConfigurationInfo(constraints = {value.setting.label: value}, values = {})]

value = rule(
    impl = _value_impl,
    attrs = {"setting": attrs.dep(providers = [ConstraintSettingInfo])},
    is_configuration_rule = True,
)

def _platform_impl(ctx):
    return [
        DefaultInfo(),
        PlatformInfo(
            label = str(ctx.label.raw_target()),
            configuration = ConfigurationInfo(
                constraints = {dep[ConstraintValueInfo].setting.label: dep[ConstraintValueInfo] for dep in ctx.attrs.values},
                values = {},
            ),
        ),
    ]

platform = rule(
    impl = _platform_impl,
    attrs = {
        "values": attrs.list(attrs.dep(providers = [ConstraintValueInfo])),
    },
    is_configuration_rule = True,
)

def _probe_impl(ctx):
    return [DefaultInfo(default_output = ctx.actions.write_json("selected.json", ctx.attrs.selected, has_content_based_path = False))]

probe = rule(
    impl = _probe_impl,
    attrs = dict(constraint_overrides.attributes, selected = attrs.list(attrs.string())),
    cfg = constraint_overrides.transition,
)

python_probe = rule(
    impl = _probe_impl,
    attrs = dict(
        constraint_overrides.attributes,
        selected = attrs.list(attrs.string()),
        omnibus_linking = attrs.option(attrs.enum(["always", "static_link_style"]), default = None),
        opt_by_default_enabled = attrs.bool(default = False),
    ),
    cfg = constraint_overrides.python_transition,
)

def _group_impl(ctx):
    return [DefaultInfo(sub_targets = {key: [DefaultInfo(), dep[ConstraintValueInfo]] for key, dep in ctx.attrs.entries.items()})]

group = rule(
    impl = _group_impl,
    attrs = {"entries": attrs.dict(attrs.string(), attrs.dep(providers = [ConstraintValueInfo]))},
    is_configuration_rule = True,
)
