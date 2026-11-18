# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@fbcode_macros//build_defs:platform_utils.bzl", "platform_utils")
load("@fbcode_macros//build_defs/lib:oss.bzl", "translate_target")
load("@prelude//decls:common.bzl", "buck")
load("@prelude//os_lookup:defs.bzl", "Os", "OsLookup")

def _buck2_bundle_impl(ctx: AnalysisContext) -> list[Provider]:
    """
    Produce a directory layout that is similar to the one our release binary
    uses, this allows setting a path for Tpx relative to BUCK2_BINARY_DIR.
    """
    target_is_windows = ctx.attrs._target_os_type[OsLookup].os == Os("windows")

    binary_extension = ".exe" if target_is_windows else ""
    buck2_binary = "buck2" + binary_extension
    buck2_tpx_binary = "buck2-tpx" + binary_extension
    buck2_daemon_binary = "buck2-daemon" + binary_extension
    buck2_health_check_binary = "buck2-health-check" + binary_extension

    copied_dir = {}
    materialisations = []

    buck2 = ctx.attrs.buck2[DefaultInfo].default_outputs[0]
    copied_dir[buck2_daemon_binary] = buck2
    materialisations.extend(ctx.attrs.buck2[DefaultInfo].other_outputs)

    buck2_client = ctx.attrs.buck2_client[DefaultInfo].default_outputs[0]
    copied_dir[buck2_binary] = buck2_client
    materialisations.extend(ctx.attrs.buck2_client[DefaultInfo].other_outputs)

    if ctx.attrs.buck2_health_check:
        buck2_health_check = ctx.attrs.buck2_health_check[DefaultInfo].default_outputs[0]
        copied_dir[buck2_health_check_binary] = buck2_health_check
        materialisations.extend(ctx.attrs.buck2_health_check[DefaultInfo].other_outputs)

    if ctx.attrs.tpx:
        tpx = ctx.attrs.tpx[DefaultInfo].default_outputs[0]
        copied_dir[buck2_tpx_binary] = ctx.actions.symlink_file(buck2_tpx_binary, tpx, has_content_based_path = False)
        materialisations.extend(ctx.attrs.tpx[DefaultInfo].other_outputs)

    out = ctx.actions.copied_dir("out", copied_dir, has_content_based_path = False)

    return [DefaultInfo(out, other_outputs = materialisations), RunInfo(cmd_args(out.project("buck2" + binary_extension), hidden = materialisations))]

_buck2_bundle = rule(
    impl = _buck2_bundle_impl,
    attrs = {
        "buck2": attrs.dep(),
        "buck2_client": attrs.dep(),
        "buck2_health_check": attrs.option(attrs.dep(), default = None),
        "labels": attrs.list(attrs.string(), default = []),
        "tpx": attrs.option(attrs.dep(), default = None),
        "_target_os_type": buck.target_os_type_arg(),
    },
)

def buck2_bundle(buck2, buck2_client, buck2_health_check, tpx, **kwargs):
    cxx_platform = platform_utils.get_cxx_platform_for_base_path(native.package_name())
    _buck2_bundle(
        buck2 = translate_target(buck2),
        buck2_client = translate_target(buck2_client),
        # @oss-disable[end= ]: buck2_health_check = buck2_health_check,
        # @oss-disable[end= ]: tpx = tpx,
        default_target_platform = cxx_platform.target_platform,
        **kwargs,
    )

def _pagable_transition_impl(platform: PlatformInfo, refs: struct) -> PlatformInfo:
    val = refs.val[ConstraintValueInfo]
    new_cfg = ConfigurationInfo(
        constraints = platform.configuration.constraints | {val.setting.label: val},
        values = platform.configuration.values,
    )
    return PlatformInfo(
        label = platform.label,
        configuration = new_cfg,
    )

_pagable_transition = transition(
    impl = _pagable_transition_impl,
    refs = {
        "val": translate_target("//buck2/starlark-rust/starlark:pagable[enabled]"),
    },
)

def _pagable_alias_impl(ctx: AnalysisContext) -> list[Provider]:
    return ctx.attrs.actual.providers

_pagable_transition_alias = rule(
    impl = _pagable_alias_impl,
    attrs = {
        "actual": attrs.dep(),
        "labels": attrs.list(attrs.string(), default = []),
    },
    cfg = _pagable_transition,
)

def pagable_transition_alias(name: str, actual, labels, modifiers, visibility = None):
    platform = platform_utils.get_cxx_platform_for_base_path(native.package_name())
    default_target_platform = platform.target_platform
    _pagable_transition_alias(
        name = name,
        actual = translate_target(actual),
        labels = labels,
        default_target_platform = default_target_platform,
        modifiers = modifiers,
        visibility = visibility,
    )
