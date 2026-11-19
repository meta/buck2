# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# The smallest cfg constructor that makes modifiers do something, for tests of how buck2 itself
# handles them (`?modifier` target patterns, `--modifier`, bxl `modifiers = [...]`). Each modifier
# must be the label of a `constraint_value`; they are applied in order on top of the legacy target
# platform, with later modifiers to the same `constraint_setting` winning.
#
# Tests of actual modifier semantics belong with the prelude's implementation, not here.

_Params = record(
    legacy_platform = PlatformInfo | None,
    modifiers = list[str],
)

def _pre_constraint_analysis(
    *, legacy_platform: PlatformInfo | None, package_modifiers, target_modifiers: list[str] | None, cli_modifiers: list[str], **_kwargs
) -> (list[str], _Params):
    if package_modifiers:
        fail("The nano prelude's cfg constructor does not support PACKAGE modifiers")
    modifiers = (target_modifiers or []) + cli_modifiers
    return modifiers, _Params(legacy_platform = legacy_platform, modifiers = modifiers)

def _post_constraint_analysis(*, refs: dict[str, ProviderCollection], params: _Params) -> PlatformInfo:
    if params.legacy_platform:
        constraints = dict(params.legacy_platform.configuration.constraints)
        values = params.legacy_platform.configuration.values
    else:
        constraints = {}
        values = {}
    for modifier in params.modifiers:
        value = refs[modifier][ConstraintValueInfo]
        constraints[value.setting.label] = value
    return PlatformInfo(
        label = "<modifiers>",
        configuration = ConfigurationInfo(constraints = constraints, values = values),
    )

def set_nano_cfg_constructor():
    set_cfg_constructor(
        stage0 = _pre_constraint_analysis,
        stage1 = _post_constraint_analysis,
        # Only used to find PACKAGE modifiers, which are rejected above
        key = "buck.cfg_modifiers",
    )
