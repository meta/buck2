# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//apple/user:apple_ipa_package.bzl", "make_apple_ipa_package_target")
load("@prelude//utils:selects.bzl", "selects")
load(":apple_bundle_config.bzl", "apple_bundle_config")
load(":apple_info_plist_substitutions_parsing.bzl", "parse_codesign_entitlements")
load(":apple_package_config.bzl", "apple_package_config")
load(
    ":apple_rules_impl_utility.bzl",
    "APPLE_ARCHIVE_OBJECTS_LOCALLY_OVERRIDE_ATTR_NAME",
)

AppleBuckConfigAttributeOverride = record(
    name = field(str),
    section = field(str, default = "apple"),
    key = field(str),
    positive_values = field([list[str], list[bool]], default = ["True", "true"]),
    value_if_true = field([str, bool, None], default = True),
    value_if_false = field([str, bool, None], default = False),
    skip_if_false = field(bool, default = False),
)

APPLE_LINK_LIBRARIES_LOCALLY_OVERRIDE = AppleBuckConfigAttributeOverride(
    name = "link_execution_preference",
    key = "link_libraries_locally_override",
    value_if_true = "local",
    skip_if_false = True,
)

APPLE_LINK_LIBRARIES_REMOTELY_OVERRIDE = AppleBuckConfigAttributeOverride(
    name = "link_execution_preference",
    key = "link_libraries_remotely_override",
    value_if_true = "remote",
    skip_if_false = True,
)

_APPLE_LIBRARY_LOCAL_EXECUTION_OVERRIDES = [
    APPLE_LINK_LIBRARIES_LOCALLY_OVERRIDE,
    APPLE_LINK_LIBRARIES_REMOTELY_OVERRIDE,
    AppleBuckConfigAttributeOverride(name = APPLE_ARCHIVE_OBJECTS_LOCALLY_OVERRIDE_ATTR_NAME, key = "archive_objects_locally_override"),
]

# If both configs are set the last one wins
_APPLE_BINARY_EXECUTION_OVERRIDES = [
    AppleBuckConfigAttributeOverride(
        name = "link_execution_preference",
        key = "link_binaries_locally_override",
        value_if_true = "local",
        skip_if_false = True,
    ),
    AppleBuckConfigAttributeOverride(
        name = "link_execution_preference",
        key = "link_binaries_remotely_override",
        value_if_true = "remote",
        skip_if_false = True,
    ),
]

_APPLE_TEST_LOCAL_EXECUTION_OVERRIDES = [
    APPLE_LINK_LIBRARIES_LOCALLY_OVERRIDE,
    APPLE_LINK_LIBRARIES_REMOTELY_OVERRIDE,
]

def apple_macro_layer_set_bool_override_attrs_from_config(overrides: list[AppleBuckConfigAttributeOverride]) -> dict[str, Select]:
    attribs = {}
    for override in overrides:
        config_value = read_root_config(override.section, override.key, None)
        if config_value != None:
            config_is_true = config_value in override.positive_values
            if not config_is_true and override.skip_if_false:
                continue
            attribs[override.name] = select({
                "DEFAULT": override.value_if_true if config_is_true else override.value_if_false,
                # Do not set attribute value for host tools
                "ovr_config//platform/execution/constraints:execution-platform-transitioned": None,
            })
    return attribs

def apple_test_macro_impl(apple_test_rule, **kwargs):
    kwargs.update(apple_bundle_config())
    kwargs.update(apple_macro_layer_set_bool_override_attrs_from_config(_APPLE_TEST_LOCAL_EXECUTION_OVERRIDES))

    kwargs["extension"] = kwargs.pop("extension", "xctest")

    apple_test_rule(**kwargs)

def apple_xcuitest_macro_impl(apple_xcuitest_rule, **kwargs):
    kwargs.update(apple_bundle_config())
    apple_xcuitest_rule(**kwargs)

def apple_bundle_macro_impl(apple_bundle_rule, **kwargs):
    info_plist_substitutions = kwargs.get("info_plist_substitutions")
    kwargs.update(apple_bundle_config())
    codesign_entitlements = selects.apply(info_plist_substitutions, parse_codesign_entitlements)

    apple_bundle_rule(_codesign_entitlements = codesign_entitlements, **kwargs)

def apple_library_macro_impl(apple_library_rule = None, **kwargs):
    kwargs.update(apple_macro_layer_set_bool_override_attrs_from_config(_APPLE_LIBRARY_LOCAL_EXECUTION_OVERRIDES))
    apple_library_rule(**kwargs)

def apple_metal_library_macro_impl(apple_metal_library_rule = None, **kwargs):
    if "exec_compatible_with" in kwargs:
        fail("Cannot set `exec_compatible_with` for apple_metal_library()")
    kwargs["exec_compatible_with"] = [
        # It's possible a buck2 daemon host machine has the correct Xcode
        # version selected but does not have the Metal toolchain installed.
        # Because this would lead to a build failure, we have to send actions
        # remotely where we it's guaranteed the Metal toolchain will be
        # installed alongside Xcode.
        "prelude//platforms:runs_only_remote",
    ]
    apple_metal_library_rule(**kwargs)

def apple_library_for_distribution_macro_impl(apple_library_for_distribution_rule = None, **kwargs):
    apple_library_macro_impl(apple_library_rule = apple_library_for_distribution_rule, **kwargs)

def prebuilt_apple_framework_macro_impl(prebuilt_apple_framework_rule = None, **kwargs):
    prebuilt_apple_framework_rule(**kwargs)

def apple_binary_macro_impl(apple_binary_rule = None, apple_universal_executable = None, **kwargs):
    kwargs.update(apple_macro_layer_set_bool_override_attrs_from_config(_APPLE_BINARY_EXECUTION_OVERRIDES))

    original_binary_name = kwargs.pop("name")

    if kwargs.pop("supports_universal", False):
        binary_name = original_binary_name + "ThinBinary"
        apple_universal_executable(
            name = original_binary_name,
            executable = ":" + binary_name,
            executable_name = original_binary_name,
            labels = kwargs.get("labels"),
            visibility = kwargs.get("visibility"),
            default_target_platform = kwargs.get("default_target_platform"),
        )
    else:
        binary_name = original_binary_name

    apple_binary_rule(name = binary_name, **kwargs)

def apple_package_macro_impl(apple_package_rule = None, apple_ipa_package_rule = None, **kwargs):
    kwargs.update(apple_package_config())
    apple_package_rule(_ipa_package = make_apple_ipa_package_target(apple_ipa_package_rule, **kwargs), **kwargs)

def apple_universal_executable_macro_impl(apple_universal_executable_rule = None, **kwargs):
    apple_universal_executable_rule(**kwargs)
