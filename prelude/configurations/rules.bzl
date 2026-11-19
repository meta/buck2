# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "make_rules")
load(
    ":defs.bzl",
    config_setting_spec = "config_setting",
    configuration_alias_spec = "configuration_alias",
    constraint_setting_spec = "constraint_setting",
    constraint_spec = "constraint",
    constraint_value_spec = "constraint_value",
    exec_platform_marker_constraint_spec = "exec_platform_marker_constraint",
    platform_spec = "platform",
)

specs = {
    "config_setting": config_setting_spec,
    "configuration_alias": configuration_alias_spec,
    "constraint": constraint_spec,
    "constraint_setting": constraint_setting_spec,
    "constraint_value": constraint_value_spec,
    "exec_platform_marker_constraint": exec_platform_marker_constraint_spec,
    "platform": platform_spec,
}

rules = make_rules(specs)

load_symbols(rules)
