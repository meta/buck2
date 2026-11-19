# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//cfg/modifier:set_cfg_modifiers.bzl", _set_cfg_modifiers = "set_cfg_modifiers")
load("@prelude//cfg/modifier:types.bzl", "Modifier")  # @unused Used in type annotation

##########################################################
# NOTE: This file is now available in the buck2 prelude. #
#                                                        #
# You should prefer including / using that version.      #
##########################################################

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
    _set_cfg_modifiers(cfg_modifiers, extra_cfg_modifiers_per_rule)
