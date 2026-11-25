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
    constraints = platform.configuration.constraints | {val.setting.label: val}

    # A PGO build applies the daemon's profile. The client-only binary is a different program,
    # and that profile would tell the compiler that everything only the client runs is cold.
    # @oss-disable[end= ]: pgo_off = refs.pgo_off[ConstraintValueInfo]
    # @oss-disable[end= ]: constraints[pgo_off.setting.label] = pgo_off
    return PlatformInfo(
        label = platform.label,
        configuration = ConfigurationInfo(
            constraints = constraints,
            values = platform.configuration.values,
        ),
    )

_transition_func = transition(
    impl = _transition_impl,
    refs = {
        # @oss-disable[end= ]: "pgo_off": "ovr_config//build_mode/constraints:llvm-ir-pgo[off]",
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
