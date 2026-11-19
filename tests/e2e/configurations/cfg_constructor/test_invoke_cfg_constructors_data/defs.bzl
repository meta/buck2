# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

KEY = "buck.cfg_modifiers"

def set_cfg_modifiers():
    write_package_value(
        KEY,
        {"root//:test_constraint_setting": "root//:test_constraint_value"},
    )

def _cfg_constructor_pre_constraint_analysis(
    legacy_platform,
    package_modifiers: dict[str, typing.Any] | None,
    target_modifiers: dict[str, typing.Any] | None,
    cli_modifiers: list[str],
    rule_name: str,
    aliases: struct | None,
    extra_data: dict[str, typing.Any] | None,
    **kwargs,
):
    _unused = kwargs  # buildifier: disable=unused-variable
    _unused = target_modifiers  # buildifier: disable=unused-variable
    _unused = cli_modifiers  # buildifier: disable=unused-variable
    _unused = extra_data  # buildifier: disable=unused-variable

    # Include valid constraints from PACKAGE modifiers
    refs = list(package_modifiers.keys()) + list(package_modifiers.values()) if package_modifiers else []
    refs += list(target_modifiers.keys()) + list(target_modifiers.values()) if target_modifiers else []
    platform = legacy_platform or PlatformInfo(
        label = "post_constraint_analysis_test_label_unbound",
        configuration = ConfigurationInfo(
            constraints = {},
            values = {},
        ),
    )

    if aliases:
        getattr(aliases, "test")  # If `aliases` is not None, we should always be able to get `test` attr from it.

    if rule_name != "test_rule":
        fail("Expected rule name to be `test_rule`. Found `{}` instead.".format(rule_name))

    return (refs, platform)

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

_ALIASES = struct(
    test = "root//:test_constraint_value",
)

def init_cfg_constructor():
    kwargs = {
        "key": KEY,
        "stage0": _cfg_constructor_pre_constraint_analysis,
        "stage1": _cfg_constructor_post_constraint_analysis,
    }
    if not read_root_config("testing", "no_aliases", None):
        kwargs["aliases"] = _ALIASES
    set_cfg_constructor(**kwargs)
