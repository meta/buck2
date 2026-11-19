# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# TODO(scottcao): Annotate these types with comments once implementation is complete

# Metadata types for where cfg modifier is defined. We need to track this to give users error messages
# that include where the bad modifier comes from.

# Modifier defined in a PACKAGE file. We track path of that PACKAGE file.
ModifierPackageLocation = record(package_path = str)

# Modifier defined on the target in buildfile.
ModifierTargetLocation = record()

# Modifier specified via command line from the user
ModifierCliLocation = record()

# Modifier specified from buckconfig
ModifierBuckconfigLocation = record(
    section = str,
    property = str,
)

# This is a handy way of specifying a rust-style enum in Starlark.
ModifierLocation = ModifierPackageLocation | ModifierTargetLocation | ModifierCliLocation | ModifierBuckconfigLocation

# Modifier types as how they appear to the user via `set_cfg_modifier` or `cfg_modifier` function.

ModifiersMatch = dict[str, typing.Any]

Modifier = str | ModifiersMatch | None

TaggedModifiers = record(
    modifiers = list[Modifier],
    location = ModifierLocation,
    rule_name = str | None,
)

# Modifier types after analysis of configuration rules.
# There is an equivalent post-constraint-analysis type for every modifier type listed above.
# An "Info" is added to the type name to denote post-constraint-analysis version of the
# modifier type.

ModifiersMatchInfo = record(
    # should be list[(ConfigurationInfo, "ModifierInfo")] once recursive types are supported
    selector = list[(ConfigurationInfo, typing.Any)],
    default = typing.Any,  # should be "ModifierInfo" | None with recursive types
)

ModifierInfo = ConstraintValueInfo | ModifiersMatchInfo | None

# A provider for conditional modifier used by cfg constructor function when constructing the
# configuration
ConditionalModifierInfo = provider(
    fields = {
        "inner": ModifierInfo,
        "key": TargetLabel,
    }
)

BuckconfigBackedModifierInfo = provider(
    fields = {
        "post_platform_modifiers": list[ConditionalModifierInfo],
        "pre_cli_modifiers": list[ConditionalModifierInfo],
        "pre_platform_modifiers": list[ConditionalModifierInfo],
    }
)

def is_modifiers_match(modifier: Modifier) -> bool:
    if modifier == None or isinstance(modifier, str):
        return False
    if isinstance(modifier, dict):
        if modifier["_type"] != "ModifiersMatch":
            fail("Found unknown dictionary `{}` for modifier".format(modifier))
        return True
    fail("Modifier should either be None, a string, or dict. Found `{}`".format(modifier))
