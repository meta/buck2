# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _configuration_info(constraints, values, root_values = None):
    return ConfigurationInfo(
        constraints = constraints,
        values = values,
        root_values = root_values or {},
    )

def _configuration_info_union(infos):
    if len(infos) == 0:
        return _configuration_info(
            constraints = {},
            values = {},
        )
    if len(infos) == 1:
        return infos[0]
    constraints = {k: v for info in infos for (k, v) in info.constraints.items()}
    values = {k: v for info in infos for (k, v) in info.values.items()}
    root_values = {k: v for info in infos for (k, v) in info.root_values.items()}
    return _configuration_info(
        constraints = constraints,
        values = values,
        root_values = root_values,
    )

def _constraint_values_to_configuration(values):
    return _configuration_info(
        constraints = {info[ConstraintValueInfo].setting.label: info[ConstraintValueInfo] for info in values},
        values = {},
    )

util = struct(
    configuration_info = _configuration_info,
    configuration_info_union = _configuration_info_union,
    constraint_values_to_configuration = _constraint_values_to_configuration,
)
