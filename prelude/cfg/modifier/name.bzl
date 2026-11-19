# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# This is an ordered dictionary of constraint setting target to an optional transform.
# The constraint settings listed below are used to name the configuration, where the first
# constraint settings are named first in the configuration. The transform, if specified,
# can change how the name appears in the configuration. By default, if there is no transform,
# we just use name of the constraint value in the configuration name. If transform is
# specified, the transform will be applied on the existing constraint value name to return a
# new string to be used in the configuration.
# @unsorted-dict-items
NAMED_CONSTRAINT_SETTINGS = {
    # TODO(scottcao): Add OSS constraints as well
    "ovr_config//build_mode/constraints:core_build_mode": (lambda label: str(label.sub_target[0])),
    "ovr_config//os/constraints:os": (lambda label: str(label.sub_target[0])),
    "ovr_config//cpu/constraints:cpu": None,
    "ovr_config//runtime/constraints:runtime": None,
    "ovr_config//runtime/constraints:runtime_version": None,
    "ovr_config//os/sdk/apple/constraints:_": (lambda label: str(label.sub_target[0])),
    "ovr_config//os/sdk/android/ndk/constraints:version": None,
    "ovr_config//os/version/android/constraints:api-level": (lambda label: "api" + str(label.name).split("-")[-1]),
    "ovr_config//toolchain/clang/constraints:clang-toolchain-version": (lambda label: "clang" + str(label.name)),
    "ovr_config//build_mode:sanitizer_type": (lambda label: str(label.sub_target[0])),
    "fbcode//fdo/constraints:fdo": (lambda label: str(label.name)),
    "ovr_config//build_mode/default_opt_cxx:default_opt_cxx_setting": (lambda label: "opt-by-default" if str(label.sub_target[0]) == "enabled" else None),
    "ovr_config//build_mode:arvr_mode": (lambda label: "arvr" if str(label.sub_target[0]) == "enabled" else None),
}

# Mark all modifier generated configurations with a `cfg:` prefix.
# We do this so that we can easily recognize which configuration is generated
# by modifiers and query for it in Scuba.
_CFG_PREFIX = "cfg:"
_EMPTY_CFG_NAME = _CFG_PREFIX + "<empty>"

def cfg_name(cfg: ConfigurationInfo) -> str:
    """Derives a reasonable name for a ConfigurationInfo"""

    name_list = []
    constraints = {str(key): value for key, value in cfg.constraints.items()}
    for constraint_setting, transform in NAMED_CONSTRAINT_SETTINGS.items():
        constraint_name = None
        if constraint_setting in constraints:
            constraint_value_label = constraints[constraint_setting].label
            if transform:
                constraint_name = transform(constraint_value_label)
            else:
                constraint_name = str(constraint_value_label.name)
        if constraint_name:
            name_list.append(constraint_name)

    if len(name_list) == 0:
        name = _EMPTY_CFG_NAME
    else:
        name = _CFG_PREFIX + "-".join(name_list)
    return name
