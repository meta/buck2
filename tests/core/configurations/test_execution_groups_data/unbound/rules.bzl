# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

Link = exec_group()

def _constraint_impl(ctx):
    setting = ConstraintSettingInfo(label = ctx.label.raw_target())
    value = ConstraintValueInfo(setting = setting, label = ctx.label.raw_target())
    return [
        DefaultInfo(),
        value,
        ConfigurationInfo(constraints = {setting.label: value}, values = {}),
    ]

# Configuration targets, such as `select()` keys, are configured unbound, which resolves no
# execution platforms. The grouped exec dep and toolchain dep must still be deps, and analysis
# must resolve them.
constraint_with_group = rule(
    impl = _constraint_impl,
    attrs = {
        "_tool": attrs.exec_dep(default = "//unbound:tool", exec_group = Link),
        "_toolchain": attrs.toolchain_dep(default = "//unbound:toolchain", exec_group = Link),
    },
    exec_groups = [Link],
)

uses_select = rule(
    impl = lambda ctx: [DefaultInfo()],
    attrs = {"value": attrs.string()},
)
