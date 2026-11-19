# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@fbcode_macros//build_defs/lib:oss.bzl", "translate_target")

def _transition_impl(platform: PlatformInfo, refs: struct) -> PlatformInfo:
    val = refs.val[ConstraintValueInfo]
    new_cfg = ConfigurationInfo(
        constraints = platform.configuration.constraints | {val.setting.label: val},
        values = platform.configuration.values,
    )
    return PlatformInfo(
        label = platform.label,
        configuration = new_cfg,
    )

_transition_func = transition(
    impl = _transition_impl,
    refs = {
        "val": translate_target("//buck2/app/buck2:buck2_client_only_setting[build]"),
    },
)

def _rule_impl(ctx: AnalysisContext) -> list[Provider]:
    return ctx.attrs.actual.providers

buck2_client_transition_alias = rule(
    impl = _rule_impl,
    attrs = {
        "actual": attrs.dep(),
    },
    cfg = _transition_func,
)
