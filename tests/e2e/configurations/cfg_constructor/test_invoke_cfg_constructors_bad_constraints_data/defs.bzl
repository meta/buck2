# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Returns refs that aren't configuration rules
def _bad_pre_constraint_analysis(
    legacy_platform, package_modifiers: list[typing.Any] | None, target_modifiers: list[typing.Any] | None, cli_modifiers: list[str], **_kwargs
):
    _unused = package_modifiers  # buildifier: disable=unused-variable
    _unused = target_modifiers  # buildifier: disable=unused-variable
    _unused = cli_modifiers  # buildifier: disable=unused-variable
    platform = legacy_platform or PlatformInfo(
        label = "post_constraint_analysis_test_label_unbound",
        configuration = ConfigurationInfo(
            constraints = {},
            values = {},
        ),
    )
    return (["root//:not_a_constraint"], platform)

def _cfg_constructor_post_constraint_analysis(refs: dict[str, ProviderCollection], params):
    _unused = refs  # buildifier: disable=unused-variable
    _unused = params  # buildifier: disable=unused-variable
    return PlatformInfo(
        label = "post_constraint_analysis_test_label",
        configuration = ConfigurationInfo(
            constraints = params.configuration.constraints,
            values = {},
        ),
    )

_ALIASES = struct()

def init_cfg_constructor():
    set_cfg_constructor(
        stage0 = _bad_pre_constraint_analysis,
        stage1 = _cfg_constructor_post_constraint_analysis,
        key = "buck.cfg_modifiers",
        aliases = _ALIASES,
    )
