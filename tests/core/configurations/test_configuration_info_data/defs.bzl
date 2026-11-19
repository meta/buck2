# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# A constraint setting whose values are its subtargets, like the prelude's `constraint()` rule
def _constraint_impl(ctx: AnalysisContext) -> list[Provider]:
    label = ctx.label.raw_target()
    setting = ConstraintSettingInfo(
        label = label,
        default = label.with_sub_target(ctx.attrs.default) if ctx.attrs.default else None,
    )
    sub_targets = {}
    for name in ctx.attrs.values:
        value = ConstraintValueInfo(setting = setting, label = label.with_sub_target(name))
        sub_targets[name] = [
            DefaultInfo(),
            value,
            ConfigurationInfo(constraints = {label: value}, values = {}),
        ]
    return [DefaultInfo(sub_targets = sub_targets), setting]

constraint = rule(
    impl = _constraint_impl,
    is_configuration_rule = True,
    attrs = {
        "default": attrs.option(attrs.string(), default = None),
        "values": attrs.list(attrs.string()),
    },
)

def _stub_impl(ctx: AnalysisContext) -> list[Provider]:
    _ignore = ctx  # buildifier: disable=unused-variable
    return [DefaultInfo()]

probe = rule(
    impl = _stub_impl,
    attrs = {"mode": attrs.string()},
)

def _force_opt_impl(platform: PlatformInfo, refs: struct) -> PlatformInfo:
    cfg = platform.configuration
    cfg.insert(refs.opt[ConstraintValueInfo])
    return PlatformInfo(label = "<opt>", configuration = cfg)

force_opt = transition(
    impl = _force_opt_impl,
    refs = {"opt": "root//:build_mode[opt]"},
)

with_transitioned_dep = rule(
    impl = _stub_impl,
    attrs = {
        "dep": attrs.dep(),
        "opt_dep": attrs.transition_dep(cfg = force_opt),
    },
)
