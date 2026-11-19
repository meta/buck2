# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

G = exec_group()

_anon = anon_rule(impl = lambda ctx: [DefaultInfo()], attrs = {}, artifact_promise_mappings = {})

def _constraint_impl(ctx):
    setting = ConstraintSettingInfo(label = ctx.label.raw_target())
    value = ConstraintValueInfo(setting = setting, label = ctx.label.raw_target())
    if ctx.attrs.anon:
        ctx.actions.anon_target(_anon, {}, exec_group = G)
    else:
        out = ctx.actions.declare_output("out")
        ctx.actions.run(["true", out.as_output()], category = "compile", exec_group = G)
    return [
        DefaultInfo(),
        value,
        ConfigurationInfo(constraints = {setting.label: value}, values = {}),
    ]

# A `select()` key is configured unbound, which resolves no execution platforms, so no group has a
# platform to run an action or anon target on.
constraint_with_group = rule(
    impl = _constraint_impl,
    attrs = {"anon": attrs.bool()},
    exec_groups = [G],
)

uses_select = rule(
    impl = lambda ctx: [DefaultInfo()],
    attrs = {"value": attrs.string()},
)
