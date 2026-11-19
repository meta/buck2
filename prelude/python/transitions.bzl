# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""
Apply python specific constraints such as opt-by-default
"""

load("@prelude//cfg/modifier:name.bzl", "cfg_name")

# This is used in prelude//transitions:constraints_override.bzl
def _transition_opt_by_default_impl(platform: PlatformInfo, refs: struct, attrs: struct) -> PlatformInfo:
    constraints = platform.configuration.constraints
    linux_key = refs._opt_by_default__linux[ConstraintValueInfo].setting.label

    # if not linux cancel the transition
    if linux_key not in constraints or constraints[linux_key].label != refs._opt_by_default__linux[ConstraintValueInfo].label:
        return platform

    if refs._opt_by_default__opt[ConstraintValueInfo].setting.label not in constraints:
        return platform

    # if this is an execution platform then cancel the transition
    if refs._opt_by_default__execution_platform_marker[ConstraintValueInfo].setting.label in constraints:
        return platform

    # check if native debug is enabled, if so cancel the transition
    maybe_native_debug_constraints = getattr(constraints.get(refs._opt_by_default_native_debug_enabled[ConstraintValueInfo].setting.label), "label", None)
    if maybe_native_debug_constraints == refs._opt_by_default_native_debug_enabled[ConstraintValueInfo].label:
        return platform

    mode_constraint = constraints[refs._opt_by_default__opt[ConstraintValueInfo].setting.label].label
    is_dev = mode_constraint == refs._opt_by_default__dev[ConstraintValueInfo].label

    # This transition upgrades dev to opt: cancel it unless the build mode is
    # dev. In particular, an opt configuration is left untouched so that python
    # targets and their dependencies share the configuration -- and its
    # outputs -- with everything else built in that opt configuration.
    if not is_dev:
        return platform

    # Preserve explicitly requested coverage (dev-cov/dbgo-cov/opt-cov) without
    # adding a ref: minimal fixtures lack the build_mode setting, and resolving
    # a missing ref fails analysis before this runs.
    for constraint_value in constraints.values():
        if str(getattr(constraint_value, "label", "")).endswith("[code-coverage]"):
            return platform

    no_san_label = refs._opt_by_default__no_san[ConstraintValueInfo].setting.label
    sanitizer_constraint = constraints[no_san_label].label if no_san_label in constraints else None
    is_default_dev_sanitizer = (
        sanitizer_constraint == refs._opt_by_default__dev_san[ConstraintValueInfo].label
    )  # this bad boy only shows up in default dev mode 🙏
    is_no_san = sanitizer_constraint == refs._opt_by_default__no_san[ConstraintValueInfo].label

    if not (is_default_dev_sanitizer or is_no_san):
        # dev-tsan/dev-asan etc modes still appear as dev, here we check that the user has not specifically requested sanitizers
        # returning here preserves the original behaviour of opt-by-default, but we can likely give a opt + sanitizer config here later
        return platform

    # if opt-by-default is not enabled then cancel the transition
    if not attrs.opt_by_default_enabled:
        return platform

    # Dev: apply the constraint values the opt mode sets, and nothing else, so
    # the result is as close to a plain opt configuration as possible. Values
    # an opt configuration derives from these (native linking, split debug info
    # handling) follow the same way they do in opt.
    #
    # `default_opt_cxx[enabled]` is required on top of `opt`, not redundant with
    # it: a mode file that sets `fbcode.build_mode_partially_selectified_test_label`
    # (dev-nosan and every other partially selectified mode) pins the fbcode C/C++
    # toolchain's mode at loading time, so `core_build_mode[opt]` alone leaves the
    # deps compiling with the dev mode's flags -- no `-DNDEBUG` -- while targets
    # that select on the opt constraint themselves do get it, which breaks the
    # link (e.g. rocksdb's `TEST_SYNC_POINT`).
    #
    # The default python package style is deliberately not forced to the opt
    # value: the python target itself is configured with the transitioned
    # configuration, so that would also repackage dev python binaries as
    # standalone.
    opt_by_default_constraints = [
        refs._opt_by_default__fbcode_build_info_mode_full[ConstraintValueInfo],
        refs._opt_by_default__static[ConstraintValueInfo],
        refs._opt_by_default__opt_cxx_enabled[ConstraintValueInfo],
        refs._opt_by_default__no_san[ConstraintValueInfo],
        refs._opt_by_default__opt[ConstraintValueInfo],
    ]
    for constraint in opt_by_default_constraints:
        constraints[constraint.setting.label] = constraint

    new_cfg = ConfigurationInfo(
        constraints = constraints,
        values = platform.configuration.values,
    )

    return PlatformInfo(
        label = cfg_name(new_cfg),
        configuration = new_cfg,
    )

def _transition_omnibus_large_code_model_impl(platform: PlatformInfo, refs: struct, attrs: struct) -> PlatformInfo:
    omnibus_linking = getattr(attrs, "omnibus_linking", None)
    if omnibus_linking == None:
        return platform

    constraints = platform.configuration.constraints
    x86_64 = refs._omnibus__x86_64[ConstraintValueInfo]
    cpu = constraints.get(x86_64.setting.label)
    if cpu == None or cpu.label != x86_64.label:
        return platform

    if omnibus_linking == "static_link_style":
        static = refs._omnibus__static[ConstraintValueInfo]
        link_style = constraints.get(static.setting.label)
        if link_style == None or link_style.label not in [static.label, refs._omnibus__static_pic[ConstraintValueInfo].label]:
            return platform
        arvr = refs._omnibus__arvr_enabled[ConstraintValueInfo]
        arvr_mode = constraints.get(arvr.setting.label)
        if arvr_mode != None and arvr_mode.label == arvr.label:
            return platform

    mcmodel_large = refs._omnibus__mcmodel_large[ConstraintValueInfo]
    mcmodel = constraints.get(mcmodel_large.setting.label)
    if mcmodel != None and mcmodel.label == mcmodel_large.label:
        return platform

    constraints = {label: constraint for label, constraint in constraints.items()}
    constraints[mcmodel_large.setting.label] = mcmodel_large
    new_cfg = ConfigurationInfo(
        constraints = constraints,
        values = platform.configuration.values,
    )

    return PlatformInfo(
        label = cfg_name(new_cfg),
        configuration = new_cfg,
    )

def _refs():
    return {
        "_omnibus__arvr_enabled": "@config//build_mode:arvr_mode[enabled]",
        "_omnibus__mcmodel_large": "@config//toolchain/cxx/constraints:mcmodel-large",
        "_omnibus__static": "@config//build_mode/constraints:default_link_style[static]",
        "_omnibus__static_pic": "@config//build_mode/constraints:default_link_style[static_pic]",
        "_omnibus__x86_64": "@config//cpu/constraints:x86_64",
        "_opt_by_default__dev": "@config//build_mode/constraints:dev",
        "_opt_by_default__dev_san": "@config//build_mode:sanitizer_type[asan-ubsan-dev]",
        "_opt_by_default__execution_platform_marker": "@config//platform/execution/constraints:execution-platform-transitioned",
        "_opt_by_default__fbcode_build_info_mode_full": "@config//build_mode/constraints:fbcode-build-info-mode[full]",
        "_opt_by_default__linux": "@config//os/constraints:linux",
        "_opt_by_default__no_san": "@config//build_mode:sanitizer_type[no-san]",
        "_opt_by_default__opt": "@config//build_mode/constraints:opt",
        "_opt_by_default__opt_cxx_enabled": "@config//build_mode/default_opt_cxx:enabled",
        "_opt_by_default__static": "@config//build_mode/constraints:default_link_style[static]",
        "_opt_by_default_native_debug_enabled": "@config//build_mode/constraints:native-debugging[supported]",
    }

def _attrs():
    return {
        "omnibus_linking": attrs.option(attrs.enum(["always", "static_link_style"]), default = None),
        "opt_by_default_enabled": attrs.bool(default = False),
    }

python_transitions = struct(
    transition_omnibus_large_code_model_impl = _transition_omnibus_large_code_model_impl,
    transition_opt_by_default_impl = _transition_opt_by_default_impl,
    attrs = _attrs,
    refs = _refs,
)
