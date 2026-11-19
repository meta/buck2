# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(":common.bzl", "get_tagged_modifiers", "tagged_modifiers_to_json")
load(":types.bzl", "Modifier", "ModifierPackageLocation")  # @unused Used in type annotation

def set_cfg_modifiers(cfg_modifiers: list[Modifier] | None = None, extra_cfg_modifiers_per_rule: dict[str, list[Modifier]] | None = None):
    """
    Sets a configuration modifier for all targets under this PACKAGE file. This can only be called from a PACKAGE file context
    (e.g. a PACKAGE file or a bzl file transitively loaded by a PACKAGE file).

    Args:
        cfg_modifiers:
            A list of modifiers to set. The simplest modifier is a constraint value target.
            For example, to change the OS to linux in fbsource, this can be specified as `["ovr_config//os/constraints:linux"]`.
        extra_cfg_modifiers_per_rule:
            A dictionary of rule name to a list of modifiers to set. This is applied on top of modifiers from `cfg_modifiers` parameter
            if a target's rule name matches the key, so it can override any modifier from `cfg_modifiers` parameter in the same PACKAGE.
            For example, if this dictionary is `{"python_binary": ["ovr_config//os/constraints:macos"]}`,
            then all python_binary targets covered will have the macos constraint added to their configurations.
    """

    # To ensure that modifiers set in PACKAGE files are easily codemoddable
    # We want to enforce that `set_cfg_modifiers` is only invokable from a PACKAGE file and not a bzl file
    frame1 = call_stack_frame(1)
    if not _is_buck_tree_file(frame1.module_path):
        # Now check the old bzl file for `set_cfg_modifiers` in case it is invoked through that one.
        frame2 = call_stack_frame(2)
        if not (frame2 and frame1.module_path.endswith("fbcode/buck2/cfg/experimental/set_cfg_modifiers.bzl") and _is_buck_tree_file(frame2.module_path)):
            if not "third-party-buck" in frame2.module_path:
                fail("set_cfg_modifiers is only allowed to be used from a PACKAGE or BUCK_TREE file, not a bzl file.")

    cfg_modifiers = cfg_modifiers or []
    extra_cfg_modifiers_per_rule = extra_cfg_modifiers_per_rule or {}

    merged_modifier_jsons = get_parent_modifiers()

    # `get_parent_modifiers` returns immutable values. `list()` makes it mutable.
    merged_modifier_jsons = list(merged_modifier_jsons) if merged_modifier_jsons else []

    tagged_modifiers_list = get_tagged_modifiers(
        cfg_modifiers,
        extra_cfg_modifiers_per_rule,
        ModifierPackageLocation(package_path = _get_package_path()),
    )
    merged_modifier_jsons += [tagged_modifiers_to_json(tagged_modifiers) for tagged_modifiers in tagged_modifiers_list]

    set_modifiers(
        merged_modifier_jsons,
    )

def _get_package_path() -> str:
    """
    Returns the cell-relative path of the current PACKAGE file.
    Ex. `foo//bar/PACKAGE`
    """

    return "{}//{}/PACKAGE".format(get_cell_name(), get_base_path())

def _is_buck_tree_file(path: str) -> bool:
    return path.endswith(("/PACKAGE", "/BUCK_TREE")) or path in ("PACKAGE", "BUCK_TREE")
