# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _force_opt_mode_impl(platform: PlatformInfo, refs: struct) -> PlatformInfo:
    opt_value = refs.opt[ConstraintValueInfo]
    build_mode_setting = opt_value.setting

    new_constraints = platform.configuration.constraints

    new_constraints[build_mode_setting.label] = opt_value

    new_cfg = ConfigurationInfo(
        constraints = new_constraints,
        values = platform.configuration.values,
    )

    return PlatformInfo(
        label = "<transitioned-to-opt>",
        configuration = new_cfg,
    )

force_opt_mode = transition(
    impl = _force_opt_mode_impl,
    refs = {
        "opt": "root//:build_mode[opt]",
    },
)

def _force_opt_mode_impl_v2(platform: PlatformInfo, refs: struct) -> PlatformInfo:
    opt_value = refs.opt[ConstraintValueInfo]

    # new_constraints = platform.configuration.constraints_v2
    new_cfg = platform.configuration
    platform.configuration.insert(opt_value)

    new_cfg.insert(opt_value)

    return PlatformInfo(
        label = "<transitioned-to-opt-v2>",
        configuration = new_cfg,
    )

force_opt_mode_v2 = transition(
    impl = _force_opt_mode_impl_v2,
    refs = {
        "opt": "root//:build_mode[opt]",
    },
)
