# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    ":common.bzl",
    "apply_buckconfig_backed_modifiers",
    "get_and_insert_modifier_info",
    "json_to_tagged_modifiers",
    "modifier_to_refs",
    "resolve_alias",
    "resolve_configuration",
)
load(
    ":types.bzl",
    "BuckconfigBackedModifierInfo",
    "Modifier",  # @unused
    "ModifierCliLocation",
    "ModifierTargetLocation",
    "TaggedModifiers",
)

PostConstraintAnalysisParams = record(
    legacy_platform = PlatformInfo | None,
    package_modifiers = list[TaggedModifiers],
    target_modifiers = list[Modifier],
    cli_modifiers = list[Modifier],
    extra_data = struct,
    configuring_exec_dep = bool,
)

def _get_buckconfig_backed_modifiers(extra_data: struct, configuring_exec_dep: bool) -> str | None:
    # If we are configuring an exec dep, we don't want to apply any modifiers from buckconfig.
    if configuring_exec_dep:
        return None
    return getattr(extra_data, "buckconfig_backed_modifiers", None)

def _has_buckconfig_backed_modifiers(refs: dict[str, ProviderCollection], target: str | None) -> bool:
    if not target:
        return False
    info = refs[target][BuckconfigBackedModifierInfo]
    return bool(info.pre_platform_modifiers or info.post_platform_modifiers or info.pre_cli_modifiers)

def cfg_constructor_pre_constraint_analysis(
    *,
    legacy_platform: PlatformInfo | None,
    # dict[str, typing.Any] is JSON dictionary form of `TaggedModifier` passed from buck2 core
    package_modifiers: list[dict[str, typing.Any]] | None,
    # typing.Any is JSON form of modifier
    target_modifiers: list[Modifier] | None,
    cli_modifiers: list[str],
    rule_name: str,
    aliases: struct,
    extra_data: struct,
    configuring_exec_dep: bool,
    **_kwargs,
) -> (list[str], PostConstraintAnalysisParams):
    """
    First stage of cfg constructor for modifiers.

    Args:
        legacy_platform:
            PlatformInfo from legacy target platform resolution, if one is specified
        package_modifiers:
            A list of modifiers specified from all parent PACKAGE files
        target_modifiers:
            A list of modifiers specified from buildfile via `metadata` attribute.
        cli_modifiers:
            modifiers specified from `--modifier` flag, `?modifier`, or BXL
        aliases:
            A struct that contains mapping of modifier aliases to modifier.
        extra_data:
            Some extra data that is for extra logging/validation for our internal modifier implementation.
        configuring_exec_dep (bool):
            Indicates whether this target is being configured as an execution dependency (exec_dep).
            When True, this flag enables the cfg_constructor to apply exec-specific modifier resolution logic.

    Returns `(refs, PostConstraintAnalysisParams)`, where `refs` is a list of fully qualified configuration
    targets we need providers for.
    """
    package_modifiers = package_modifiers or []
    target_modifiers = target_modifiers or []

    # Convert JSONs back to TaggedModifiers
    package_modifiers = [json_to_tagged_modifiers(modifier_json) for modifier_json in package_modifiers]

    # Filter PACKAGE modifiers based on rule name.
    # This only filters out PACKAGE modifiers from `extra_cfg_modifiers_per_rule` argument of `set_cfg_modifiers` function.
    package_modifiers = [
        tagged_modifiers for tagged_modifiers in package_modifiers if tagged_modifiers.rule_name == None or tagged_modifiers.rule_name == rule_name
    ]

    # Resolve all aliases in CLI modifiers
    cli_modifiers = [resolved_modifier for modifier in cli_modifiers for resolved_modifier in resolve_alias(modifier, aliases)]

    refs = []
    buckconfig_backed_modifiers = _get_buckconfig_backed_modifiers(extra_data, configuring_exec_dep)
    if buckconfig_backed_modifiers:
        refs.append(buckconfig_backed_modifiers)

    for tagged_modifiers in package_modifiers:
        for modifier in tagged_modifiers.modifiers:
            refs.extend(modifier_to_refs(modifier, tagged_modifiers.location))
    for modifier in target_modifiers:
        refs.extend(modifier_to_refs(modifier, ModifierTargetLocation()))
    for modifier in cli_modifiers:
        refs.extend(modifier_to_refs(modifier, ModifierCliLocation()))

    return refs, PostConstraintAnalysisParams(
        legacy_platform = legacy_platform,
        package_modifiers = package_modifiers,
        target_modifiers = target_modifiers,
        cli_modifiers = cli_modifiers,
        extra_data = extra_data,
        configuring_exec_dep = configuring_exec_dep,
    )

def cfg_constructor_post_constraint_analysis(*, refs: dict[str, ProviderCollection], params: PostConstraintAnalysisParams) -> PlatformInfo:
    """
    Second stage of cfg constructor for modifiers.

    Args:
        refs: a dictionary of fully qualified target labels for configuration targets with their providers
        params: `PostConstraintAnalysisParams` returned from first stage of cfg constructor

    Returns a PlatformInfo
    """

    buckconfig_backed_modifiers = _get_buckconfig_backed_modifiers(params.extra_data, params.configuring_exec_dep)
    if not (params.package_modifiers or params.target_modifiers or params.cli_modifiers or _has_buckconfig_backed_modifiers(refs, buckconfig_backed_modifiers)):
        # If there is no modifier and legacy platform is specified,
        # then return the legacy platform as is without changing the label or
        # configuration.
        return params.legacy_platform or PlatformInfo(
            # Empty configuration
            label = "<empty>",
            configuration = ConfigurationInfo(
                constraints = {},
                values = {},
            ),
        )

    constraint_setting_to_modifier_infos = {}

    if buckconfig_backed_modifiers:
        apply_buckconfig_backed_modifiers(
            constraint_setting_to_modifier_infos, refs[buckconfig_backed_modifiers][BuckconfigBackedModifierInfo].pre_platform_modifiers
        )

    if params.legacy_platform:
        for constraint_setting, constraint_value_info in params.legacy_platform.configuration.constraints.items():
            constraint_setting_to_modifier_infos[constraint_setting] = [constraint_value_info]

    if buckconfig_backed_modifiers:
        apply_buckconfig_backed_modifiers(
            constraint_setting_to_modifier_infos, refs[buckconfig_backed_modifiers][BuckconfigBackedModifierInfo].post_platform_modifiers
        )

    for tagged_modifiers in params.package_modifiers:
        for modifier in tagged_modifiers.modifiers:
            if modifier:
                get_and_insert_modifier_info(
                    constraint_setting_to_modifier_infos = constraint_setting_to_modifier_infos,
                    refs = refs,
                    modifier = modifier,
                    location = tagged_modifiers.location,
                )

    for modifier in params.target_modifiers:
        if modifier:
            get_and_insert_modifier_info(
                constraint_setting_to_modifier_infos = constraint_setting_to_modifier_infos,
                refs = refs,
                modifier = modifier,
                location = ModifierTargetLocation(),
            )

    if buckconfig_backed_modifiers:
        apply_buckconfig_backed_modifiers(
            constraint_setting_to_modifier_infos, refs[buckconfig_backed_modifiers][BuckconfigBackedModifierInfo].pre_cli_modifiers
        )

    for modifier in params.cli_modifiers:
        if modifier:
            get_and_insert_modifier_info(
                constraint_setting_to_modifier_infos = constraint_setting_to_modifier_infos,
                refs = refs,
                modifier = modifier,
                location = ModifierCliLocation(),
            )

    return resolve_configuration(constraint_setting_to_modifier_infos)
