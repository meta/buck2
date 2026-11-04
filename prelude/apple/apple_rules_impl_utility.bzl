# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:attrs_validators.bzl", "validation_common")
load("@prelude//apple:apple_bundle_types.bzl", "AppleBundleTypeAttributeType")
load("@prelude//apple:apple_code_signing_types.bzl", "CodeSignConfiguration", "CodeSignType")
load("@prelude//apple:apple_common.bzl", "apple_common", "apple_hot_code_reload_enabled_select")
load("@prelude//apple:apple_execution_groups.bzl", "AppleResources")
load("@prelude//apple:apple_test_device_types.bzl", "AppleTestDeviceType")
load("@prelude//apple:apple_toolchain_types.bzl", "AppleToolchainInfo")
load("@prelude//apple:resource_groups.bzl", "RESOURCE_GROUP_MAP_ATTR")
load("@prelude//apple/swift:swift_incremental_support.bzl", "SwiftCompilationMode")
load("@prelude//apple/user:apple_selective_debugging.bzl", "AppleSelectiveDebuggingInfo")
load("@prelude//apple/user:cpu_split_transition.bzl", "cpu_split_transition")
load("@prelude//cxx:headers.bzl", "CPrecompiledHeaderInfo")
load("@prelude//ide_integrations/xcode:scheme_settings.bzl", "XCODE_SCHEME_SETTINGS_ATTR_NAME", "XCODE_SCHEME_SETTINGS_ATTR_TYPE")
load("@prelude//linking:execution_preference.bzl", "link_execution_preference_attr")
load("@prelude//linking:link_info.bzl", "LinkOrdering")
load("@prelude//utils:buckconfig.bzl", "read_bool")
load("@prelude//utils:clear_platform.bzl", "clear_platform_transition")

AppleFrameworkBundleModuleMapType = ["auto"]

def get_apple_bundle_toolchain_attr():
    return attrs.toolchain_dep(default = "toolchains//:apple-bundle", providers = [AppleToolchainInfo], exec_group = DefaultExecGroup)

def get_apple_resources_toolchain_attr(exec_group = None):
    # Only macOS hosted toolchains, so resources are compiled directly on Mac RE.
    return attrs.toolchain_dep(default = "toolchains//:apple-resources", providers = [AppleToolchainInfo], exec_group = exec_group)

def get_apple_xctoolchain_attr():
    return attrs.toolchain_dep(default = "toolchains//:apple-xctoolchain", exec_group = DefaultExecGroup)

def get_apple_xctoolchain_bundle_id_attr():
    return attrs.toolchain_dep(default = "toolchains//:apple-xctoolchain-bundle-id", exec_group = DefaultExecGroup)

def get_enable_library_evolution():
    return attrs.bool(
        default = select({
            "DEFAULT": False,
            "config//features/apple:swift_library_evolution_enabled": True,
        })
    )

def _strict_provisioning_profile_search_default_attr():
    default_value = read_root_config("apple", "strict_provisioning_profile_search", "true").lower() == "true"
    return attrs.bool(
        default = select({
            "DEFAULT": default_value,
            "config//features/apple:strict_provisioning_profile_search_enabled": True,
        })
    )

def _fast_adhoc_signing_enabled_default_attr():
    return attrs.bool(
        default = select({
            "DEFAULT": True,
            "config//features/apple:fast_adhoc_signing_disabled": False,
            "config//features/apple:fast_adhoc_signing_enabled": True,
        })
    )

def _skip_adhoc_resigning_scrubbed_frameworks_default_attr():
    default_value = read_root_config("apple", "skip_adhoc_resigning_scrubbed_frameworks", "").lower() == "true"
    return attrs.bool(
        default = select({
            "DEFAULT": default_value,
            "config//features/apple/constraints:skip_adhoc_resigning_scrubbed_frameworks_disabled": False,
            "config//features/apple/constraints:skip_adhoc_resigning_scrubbed_frameworks_enabled": True,
        })
    )

def _versioned_macos_bundle_default_value():
    return select({
        "DEFAULT": False,
        "config//features/apple/constraints:versioned_macos_bundle_false": False,
        "config//features/apple/constraints:versioned_macos_bundle_true": True,
    })

def get_versioned_macos_bundle_attr():
    return attrs.bool(default = _versioned_macos_bundle_default_value())

def _include_build_info_file_default_value():
    return select({
        "DEFAULT": select({
            "DEFAULT": read_root_config("apple", "include_build_info_file", "false").lower() == "true",
            # Unless explicitly requested, production builds (aka App Store/Developer ID) do not include internal build info file
            "config//build_mode/apple/constraints:build_mode[production]": False,
        }),
        "config//features/apple/constraints:include_build_info_file[disabled]": False,
        "config//features/apple/constraints:include_build_info_file[enabled]": True,
    })

APPLE_ARCHIVE_OBJECTS_LOCALLY_OVERRIDE_ATTR_NAME = "_archive_objects_locally_override"
APPLE_USE_ENTITLEMENTS_WHEN_ADHOC_CODE_SIGNING_CONFIG_OVERRIDE_ATTR_NAME = "_use_entitlements_when_adhoc_code_signing"
APPLE_USE_ENTITLEMENTS_WHEN_ADHOC_CODE_SIGNING_ATTR_NAME = "use_entitlements_when_adhoc_code_signing"
APPLE_EMBED_PROVISIONING_PROFILE_WHEN_ADHOC_CODE_SIGNING_CONFIG_OVERRIDE_ATTR_NAME = "_embed_provisioning_profile_when_adhoc_code_signing"
APPLE_EMBED_PROVISIONING_PROFILE_WHEN_ADHOC_CODE_SIGNING_ATTR_NAME = "embed_provisioning_profile_when_adhoc_code_signing"

APPLE_VALIDATION_DEPS_ATTR_NAME = "validation_deps"
APPLE_VALIDATION_DEPS_ATTR_TYPE = attrs.set(attrs.dep(), sorted = True, default = [])

def get_apple_info_plist_build_system_identification_attrs():
    return {
        "info_plist_identify_build_system": attrs.option(attrs.bool(), default = None),
        "_info_plist_identify_build_system_default": attrs.bool(default = False),
    }

def get_skip_swift_incremental_outputs_attrs():
    return {
        "_skip_swift_incremental_outputs": attrs.bool(default = read_bool("apple", "skip_swift_incremental_outputs", False, False, True)),
    }

def get_swift_incremental_file_hashing_attrs():
    return {
        "swift_incremental_file_hashing": attrs.bool(default = read_bool("apple", "swift_incremental_file_hashing", False, False, True)),
    }

def get_swift_incremental_remote_outputs_attrs():
    return {
        "incremental_remote_outputs": attrs.bool(default = read_bool("apple", "incremental_remote_outputs", False, False, True)),
    }

def get_swift_incremental_logging_attrs():
    return {
        "swift_incremental_logging": attrs.bool(default = read_bool("apple", "swift_incremental_logging_enabled", False, False, True)),
    }

def get_incremental_split_actions_attrs():
    return {
        "_swift_incremental_split_actions": attrs.bool(default = read_bool("apple", "swift_incremental_split_actions", False, False, True)),
    }

def get_incremental_swiftmodule_action_attrs():
    return {
        "_swift_incremental_swiftmodule_action": attrs.bool(default = read_bool("apple", "swift_incremental_swiftmodule_action", False, False, True)),
    }

def _apple_bundle_like_common_attrs():
    # `apple_bundle()` and `apple_test()` share a common set of extra attrs
    attribs = {
        # Target-level attribute always takes precedence over buckconfigs.
        "code_signing_configuration": attrs.option(attrs.enum(CodeSignConfiguration.values()), default = None),
        "codesign_type": attrs.option(attrs.enum(CodeSignType.values()), default = None),
        "entitlements_verification_check_enabled": attrs.bool(
            default = select({
                "DEFAULT": read_bool("apple", "entitlements_verification_check_enabled", default = True, root_cell = True),
                "config//features/apple:entitlements_verification_check_disabled": False,
                "config//features/apple:entitlements_verification_check_enabled": True,
            })
        ),
        "fast_adhoc_signing_enabled": attrs.option(attrs.bool(), default = None),
        "include_build_info_file": attrs.bool(default = _include_build_info_file_default_value()),
        "include_build_info_file_in_bundles_with_extensions": attrs.list(attrs.string(), default = ["app"]),
        "provisioning_profile_filter": attrs.option(attrs.string(), default = None),
        "skip_adhoc_resigning_scrubbed_frameworks": attrs.option(attrs.bool(), default = None),
        "strict_provisioning_profile_search": attrs.option(attrs.bool(), default = None),
        "versioned_macos_bundle": get_versioned_macos_bundle_attr(),
        "_apple_xctoolchain": get_apple_xctoolchain_attr(),
        "_apple_xctoolchain_bundle_id": get_apple_xctoolchain_bundle_id_attr(),
        "_bundling_cache_buster": attrs.option(attrs.string(), default = None),
        "_bundling_log_file_enabled": attrs.bool(default = False),
        "_bundling_log_file_level": attrs.option(attrs.string(), default = None),
        "_code_signing_configuration": attrs.option(attrs.enum(CodeSignConfiguration.values()), default = None),
        "_codesign_command_override": attrs.option(attrs.exec_dep(providers = [RunInfo], exec_group = DefaultExecGroup), default = None),
        "_codesign_identities_command_override": attrs.option(attrs.exec_dep(providers = [RunInfo], exec_group = DefaultExecGroup), default = None),
        "_codesign_type": attrs.option(attrs.enum(CodeSignType.values()), default = None),
        "_compile_resources_locally_override": attrs.option(attrs.bool(), default = None),
        "_fast_adhoc_signing_enabled_default": _fast_adhoc_signing_enabled_default_attr(),
        "_fast_adhoc_signing_probe_enabled": attrs.bool(default = True),
        "_fast_provisioning_profile_parsing_enabled": attrs.bool(default = False),
        "_incremental_bundling_enabled": attrs.bool(default = False),
        "_no_check_certificates": attrs.bool(default = False),
        "_profile_bundling_enabled": attrs.bool(default = False),
        "_provisioning_profile_sources": attrs.dep(default = "fbsource//xplat/buck2/platform/apple:provisioning_profile_sources"),
        "_skip_adhoc_resigning_scrubbed_frameworks_default": _skip_adhoc_resigning_scrubbed_frameworks_default_attr(),
        "_skip_adhoc_resigning_scrubbed_frameworks_override": attrs.option(attrs.bool(), default = None),
        "_strict_provisioning_profile_search_default": _strict_provisioning_profile_search_default_attr(),
        APPLE_USE_ENTITLEMENTS_WHEN_ADHOC_CODE_SIGNING_CONFIG_OVERRIDE_ATTR_NAME: attrs.option(attrs.bool(), default = None),
        APPLE_USE_ENTITLEMENTS_WHEN_ADHOC_CODE_SIGNING_ATTR_NAME: attrs.bool(default = False),
        APPLE_EMBED_PROVISIONING_PROFILE_WHEN_ADHOC_CODE_SIGNING_CONFIG_OVERRIDE_ATTR_NAME: attrs.option(attrs.bool(), default = None),
        APPLE_EMBED_PROVISIONING_PROFILE_WHEN_ADHOC_CODE_SIGNING_ATTR_NAME: attrs.bool(default = False),
        APPLE_VALIDATION_DEPS_ATTR_NAME: APPLE_VALIDATION_DEPS_ATTR_TYPE,
        XCODE_SCHEME_SETTINGS_ATTR_NAME: XCODE_SCHEME_SETTINGS_ATTR_TYPE,
    }
    attribs.update(get_apple_info_plist_build_system_identification_attrs())
    attribs.update(apple_common.apple_tools_arg())
    attribs.update(apple_common.enforce_minimum_os_plist_key())
    return attribs

def apple_test_extra_attrs():
    # To build an `apple_test`, one needs to first build a shared `apple_library` then
    # wrap this test library into an `apple_bundle`. Because of this, `apple_test` has attributes
    # from both `apple_library` and `apple_bundle`.
    attribs = {
        # Expected by `apple_bundle`, for `apple_test` this field is always None.
        "binary": attrs.option(attrs.dep(), default = None),
        "enable_library_evolution": attrs.option(attrs.bool(), default = None),
        # The resulting test bundle should have .xctest extension.
        "extension": attrs.string(),
        "link_execution_preference": link_execution_preference_attr(),
        "link_ordering": attrs.option(attrs.enum(LinkOrdering.values()), default = None),
        "precompiled_header": attrs.option(attrs.dep(providers = [CPrecompiledHeaderInfo]), default = None),
        # Expected by `apple_bundle`, for `apple_test` this field is always None.
        "resource_group": attrs.option(attrs.string(), default = None),
        # Expected by `apple_bundle`, for `apple_test` this field is always None.
        "resource_group_map": attrs.option(attrs.string(), default = None),
        "sanitizer_runtime_enabled": attrs.option(attrs.bool(), default = None),
        # Tests default to unstripped so they can be debugged.
        "strip_level": attrs.default_only(attrs.string(default = "unstripped")),
        "swift_compilation_mode": attrs.enum(SwiftCompilationMode.values(), default = "wmo"),
        "swift_package_name": attrs.option(attrs.string(), default = None),
        "swift_testing": attrs.bool(default = False),
        "test_device_type": attrs.enum(AppleTestDeviceType.values(), default = "default"),
        "test_re_capabilities": attrs.option(
            attrs.dict(key = attrs.string(), value = attrs.string(), sorted = False),
            default = None,
            doc = """
            An optional dictionary with the RE capabilities for the test execution.
            Overrides a default selection mechanism.
        """,
        ),
        "test_re_use_case": attrs.option(
            attrs.string(),
            default = None,
            doc = """
            An optional name of the RE use case for the test execution.
            Overrides a default selection mechanism.
        """,
        ),
        "_enable_library_evolution": get_enable_library_evolution(),
        "_ipad_simulator": attrs.transition_dep(
            cfg = clear_platform_transition, default = "fbsource//xplat/buck2/platform/apple:ipad_simulator", providers = [LocalResourceInfo]
        ),
        "_iphone_booted_simulator": attrs.transition_dep(
            cfg = clear_platform_transition, default = "fbsource//xplat/buck2/platform/apple:iphone_booted_simulator", providers = [LocalResourceInfo]
        ),
        "_iphone_unbooted_simulator": attrs.transition_dep(
            cfg = clear_platform_transition, default = "fbsource//xplat/buck2/platform/apple:iphone_unbooted_simulator", providers = [LocalResourceInfo]
        ),
        "_swift_enable_testing": attrs.default_only(attrs.bool(default = True)),
        "_watch_simulator": attrs.transition_dep(
            cfg = clear_platform_transition, default = "fbsource//xplat/buck2/platform/apple:watch_simulator", providers = [LocalResourceInfo]
        ),
    } | validation_common.attrs_validators_arg()
    attribs.update(apple_common.apple_toolchain_arg())
    attribs.update(apple_common.minimum_os_version_arg())
    attribs.update(_apple_bundle_like_common_attrs())
    attribs.update(_apple_resources_toolchain_attrs())
    attribs.update(get_swift_incremental_file_hashing_attrs())
    attribs.update(get_swift_incremental_logging_attrs())
    attribs.update(get_skip_swift_incremental_outputs_attrs())
    attribs.update(get_incremental_split_actions_attrs())
    attribs.update(get_incremental_swiftmodule_action_attrs())
    return attribs

def apple_xcuitest_extra_attrs():
    attribs = {
        # This is ignored, but required for info plist processing.
        "binary": attrs.option(attrs.source(), default = None),
        "codesign_identity": attrs.option(attrs.string(), default = None),
        "enable_library_evolution": attrs.option(attrs.bool(), default = None),
        "entitlements_file": attrs.option(attrs.source(), default = None),
        "extension": attrs.default_only(attrs.string(default = "app")),
        "incremental_bundling_enabled": attrs.bool(default = False),
        "info_plist": attrs.source(),
        "info_plist_substitutions": attrs.dict(key = attrs.string(), value = attrs.string(), sorted = False, default = {}),
        # The test bundle to package in the UI test runner app.
        "test_bundle": attrs.dep(),
        "_enable_library_evolution": get_enable_library_evolution(),
    }
    attribs.update(apple_common.target_sdk_version())
    attribs.update(apple_common.apple_toolchain_arg())
    attribs.update(_apple_bundle_like_common_attrs())

    return attribs

def _embed_xctest_frameworks_default_value():
    return select({
        "DEFAULT": False,
        # Xcode copies XCTest frameworks to test host apps, required when the
        # selected Xcode version != Xcode version used to build an app under test
        "config//marker/apple/constraints:embed_xctest_frameworks[enabled]": True,
    })

def _apple_resources_toolchain_attrs():
    # `apple_bundle()` and `apple_test()` compile resources in the `AppleResources` execution group.
    return {"_apple_resources_toolchain": get_apple_resources_toolchain_attr(exec_group = AppleResources)}

def apple_bundle_extra_attrs():
    attribs = {
        "binary": attrs.option(attrs.split_transition_dep(cfg = cpu_split_transition), default = None),
        "bundle_type": attrs.option(attrs.enum(AppleBundleTypeAttributeType.values()), default = None),
        "copy_public_framework_headers": attrs.option(attrs.bool(), default = None),
        "embed_xctest_frameworks": attrs.bool(default = _embed_xctest_frameworks_default_value()),
        "module_map": attrs.option(attrs.one_of(attrs.enum(AppleFrameworkBundleModuleMapType), attrs.source()), default = None),
        "resource_group_map": RESOURCE_GROUP_MAP_ATTR,
        "selective_debugging": attrs.option(attrs.dep(providers = [AppleSelectiveDebuggingInfo]), default = None),
        "split_arch_dsym": attrs.bool(default = False),
        "universal": attrs.option(attrs.bool(), default = None),
        "validate_rrapi_usage": attrs.bool(default = False),
        "xcassets_usage_targets": attrs.list(attrs.split_transition_dep(cfg = cpu_split_transition), default = []),
        "_apple_toolchain": get_apple_bundle_toolchain_attr(),
        "_codesign_entitlements": attrs.option(attrs.source(), default = None),
    }
    attribs.update(_apple_bundle_like_common_attrs())
    attribs.update(_apple_resources_toolchain_attrs())
    attribs.update(apple_common.entitlements_suffixed_key_map_arg())
    attribs.update(apple_common.entitlements_removed_keys_arg())
    attribs.update(apple_common.entitlements_removed_values_map_arg())
    attribs.update(apple_common.minimum_os_version_arg())
    # Only `apple_bundle`: `apple_test` shares the bundle-like attributes but
    # test code is never compiled for replacement.
    attribs.update(apple_common.swift_dynamic_replacement_enabled_arg())
    # Whether this is a hot code reload build, recorded in `BuildInfo.json`;
    # test bundles take no part in hot reload.
    attribs["_hot_code_reload_enabled"] = attrs.default_only(attrs.bool(default = apple_hot_code_reload_enabled_select(True, False)))
    return attribs
