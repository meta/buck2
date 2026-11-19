# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Utility functions used by "fdb.bxl"

load("@prelude//debugging:types.bzl", "TargetInfo")

def target_name(node: bxl.ConfiguredTargetNode | bxl.UnconfiguredTargetNode) -> str:
    return "{}:{}".format(str(node.label.path), node.label.name)

def rule_type(node: bxl.ConfiguredTargetNode) -> str:
    return node.rule_type

def rule_name(node: bxl.ConfiguredTargetNode) -> str:
    """The rule type without the file that defines the rule, e.g. `cxx_binary`."""
    return node.rule_type.rsplit(":", 1)[-1]

def create_target_info(target: bxl.ConfiguredTargetNode) -> TargetInfo:
    attrs = target.attrs_lazy()
    return TargetInfo(
        target = target_name(target),
        target_type = rule_name(target),
        labels = attrs.get("labels").value() if attrs.get("labels") != None else [],
    )
