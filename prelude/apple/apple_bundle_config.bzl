# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//utils:buckconfig.bzl", "read_bool")
load(":apple_code_signing_types.bzl", "CodeSignConfiguration")

def _read_bool(config: str, default: [None, bool] = None) -> [None, bool]:
    return read_bool("apple", config, default = default, required = False, root_cell = True)

def _get_code_signing_configuration() -> str:
    is_dry_run = _read_bool("dry_run_code_signing", False)

    # This is a kill switch for the feature, it can also be disabled by setting
    # `apple.fast_adhoc_signing_enabled=false` in a global buckconfig file.
    is_fast_adhoc_signing_enabled = _read_bool("fast_adhoc_signing_enabled", True)

    is_codesign_execution_bypass_enabled = _read_bool("codesign_execution_bypass", False)
    if is_dry_run and is_codesign_execution_bypass_enabled:
        fail("Dry run and execution bypass are mutually exclusive, pick one")

    if is_dry_run:
        return CodeSignConfiguration("dry-run").value
    elif is_codesign_execution_bypass_enabled:
        # Fast adhoc only makes sense if we're executing codesign commands,
        # hence execution bypass takes precedence
        return CodeSignConfiguration("execution-bypass").value
    elif is_fast_adhoc_signing_enabled:
        return CodeSignConfiguration("fast-adhoc").value
    else:
        return CodeSignConfiguration("none").value

def apple_bundle_config() -> dict[str, typing.Any]:
    return {
        "_bundling_cache_buster": read_root_config("apple", "bundling_cache_buster", None),
        "_bundling_log_file_enabled": _read_bool("bundling_log_file_enabled", True),
        "_bundling_log_file_level": read_root_config("apple", "bundling_log_file_level", None),
        "_code_signing_configuration": _get_code_signing_configuration(),
        "_codesign_command_override": read_root_config("apple", "codesign_command_override", None),
        "_codesign_identities_command_override": read_root_config("apple", "codesign_identities_command_override", None),
        "_codesign_type": read_root_config("apple", "codesign_type_override", None),
        "_compile_resources_locally_override": _read_bool("compile_resources_locally_override"),
        "_embed_provisioning_profile_when_adhoc_code_signing": _read_bool("embed_provisioning_profile_when_adhoc_code_signing"),
        "_fast_adhoc_signing_probe_enabled": _read_bool("fast_adhoc_signing_probe_enabled", True),
        "_fast_provisioning_profile_parsing_enabled": _read_bool("fast_provisioning_profile_parsing_enabled", False),
        "_incremental_bundling_enabled": _read_bool("incremental_bundling_enabled", True),
        "_info_plist_identify_build_system_default": _read_bool("info_plist_identify_build_system", True),
        "_no_check_certificates": _read_bool("no_check_certificates", False),
        "_profile_bundling_enabled": _read_bool("profile_bundling_enabled", False),
        "_skip_adhoc_resigning_scrubbed_frameworks_override": _read_bool("skip_adhoc_resigning_scrubbed_frameworks_override"),
        "_use_entitlements_when_adhoc_code_signing": _read_bool("use_entitlements_when_adhoc_code_signing"),
    }
