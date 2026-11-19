# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# @oss-disable[end= ]: load("@fbsource//tools/build_defs:python_platform.bzl", "set_platform_decorator_for_python")
load("@prelude//:is_full_meta_repo.bzl", "is_full_meta_repo")
load("@prelude//:native.bzl", _native = "native")

set_platform_decorator_for_python = lambda **kwargs: kwargs # @oss-enable

def meta_python_test(name, **kwargs):
    # Set the platform attributes as needed for proper exec platform resolution
    kwargs = set_platform_decorator_for_python(
        # @oss-disable[end= ]: set_python_constraint_overrides = True,
        **kwargs,
    )

    _native.python_test(name = name, **kwargs)

_PYTHON_SCRUBBER = "prelude//apple/tools/selective_debugging:tool"
_NATIVE_SCRUBBER = "prelude//apple/tools/meta_only/prebuilt_native_oso_scrubber:prebuilt_native_oso_scrubber"

def apple_oso_scrubber_target():
    if not is_full_meta_repo():
        return _PYTHON_SCRUBBER

    native_oso_scrubber_enabled_override = read_root_config("apple", "native_oso_scrubber_enabled_override", None)
    if native_oso_scrubber_enabled_override != None:
        # Override buckconfig takes precedence over everything else
        native_oso_scrubber_enabled = native_oso_scrubber_enabled_override.lower() == "true"
        return _NATIVE_SCRUBBER if native_oso_scrubber_enabled else _PYTHON_SCRUBBER

    native_oso_scrubber_enabled = read_root_config("apple", "native_oso_scrubber_enabled", "").lower() == "true"
    default_scrubber = _NATIVE_SCRUBBER if native_oso_scrubber_enabled else _PYTHON_SCRUBBER

    return select({
        "DEFAULT": default_scrubber,
        "ovr_config//features/apple/constraints:native_oso_scrubber[disabled]": _PYTHON_SCRUBBER,
        "ovr_config//features/apple/constraints:native_oso_scrubber[enabled]": _NATIVE_SCRUBBER,
    })

def bundle_telemetry_logger_target():
    return read_root_config("apple", "bundle_telemetry_logger", None)

def swiftmodule_change_analysis_target():
    # Overridable via buckconfig either way.
    return read_root_config("apple", "swiftmodule_change_analysis", None)
