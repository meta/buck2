# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

KEY = "buck.cfg_modifiers"

def _cfg_constructor_pre_constraint_analysis(
        legacy_platform,
        package_modifiers: dict[str, typing.Any] | None,
        target_modifiers: dict[str, typing.Any] | None,
        cli_modifiers: list[str],
        rule_name: str,
        aliases: struct | None,
        extra_data: dict[str, typing.Any] | None):
    _unused = package_modifiers  # buildifier: disable=unused-variable
    _unused = target_modifiers  # buildifier: disable=unused-variable
    _unused = cli_modifiers  # buildifier: disable=unused-variable
    _unused = extra_data  # buildifier: disable=unused-variable
    _unused = aliases  # buildifier: disable=unused-variable
    _unused = rule_name  # buildifier: disable=unused-variable

    platform = legacy_platform or PlatformInfo(label = "root_cfg_constructor", configuration = ConfigurationInfo(
        constraints = {},
        values = {},
    ))
    return ([], platform)

def _cfg_constructor_post_constraint_analysis(refs: dict[str, ProviderCollection], params):
    _unused = refs  # buildifier: disable=unused-variable
    _unused = params  # buildifier: disable=unused-variable
    return PlatformInfo(label = "root_post_constraint_analysis", configuration = ConfigurationInfo(
        constraints = params.configuration.constraints,
        values = {},
    ))

def init_cfg_constructor():
    set_cfg_constructor(
        key = KEY,
        stage0 = _cfg_constructor_pre_constraint_analysis,
        stage1 = _cfg_constructor_post_constraint_analysis,
    )
