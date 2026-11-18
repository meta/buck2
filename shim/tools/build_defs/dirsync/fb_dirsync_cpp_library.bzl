# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    "@fbsource//tools/build_defs:platform_defs.bzl",
    "ANDROID",
    "APPLE",
    "APPLETVOS",
    "CXX",
    "FBCODE",
    "IOS",
    "MACOSX",
    "WINDOWS",
    "xplat_target_compatible_with",
)
load("@prelude//utils:selects.bzl", "selects")
load("@shim//build_defs:cpp_library.bzl", "cpp_library")

def fb_dirsync_cpp_library(
    name,
    apple_sdks = (IOS, APPLETVOS, MACOSX),
    cxx_deps = None,
    cxx_exported_deps = None,
    enable_static_variant = None,
    exported_headers = None,
    fbandroid_compiler_flags = None,
    fbandroid_deps = None,
    fbandroid_exported_deps = None,
    fbandroid_exported_preprocessor_flags = None,
    fbandroid_abi_variants = None,
    fbandroid_preprocessor_flags = None,
    fbandroid_use_host_platform = None,
    fbobjc_compiler_flags = None,
    fbobjc_complete_nullability = None,
    fbobjc_exported_preprocessor_flags = None,
    fbobjc_preprocessor_flags = None,
    header_namespace = None,
    platforms = (ANDROID, APPLE, CXX, FBCODE, WINDOWS),
    raw_headers = None,
    raw_headers_as_headers_mode = None,
    use_export_header_unit = None,
    use_raw_headers = None,
    windows_exported_linker_flags = None,
    windows_preferred_linkage = None,
    xplat_compiler_flags = None,
    xplat_impl = None,
    **kwargs,
):
    _unused = (
        enable_static_variant,
        fbandroid_abi_variants,
        fbandroid_use_host_platform,
        fbobjc_complete_nullability,
        raw_headers_as_headers_mode,
        use_export_header_unit,
        use_raw_headers,
        windows_preferred_linkage,
        xplat_impl,
    )  # @unused
    if windows_exported_linker_flags:
        kwargs["exported_linker_flags"] = kwargs.pop("exported_linker_flags", []) + select({
            "DEFAULT": [],
            "ovr_config//os:windows": windows_exported_linker_flags,
        })
    if header_namespace != None:
        kwargs["header_namespace"] = header_namespace
    if xplat_compiler_flags:
        kwargs["compiler_flags"] = kwargs.get("compiler_flags", []) + xplat_compiler_flags
    kwargs["compiler_flags"] = kwargs.get("compiler_flags", []) + _platform_value(fbandroid_compiler_flags, fbobjc_compiler_flags, [])
    kwargs["preprocessor_flags"] = kwargs.get("preprocessor_flags", []) + _platform_value(fbandroid_preprocessor_flags, fbobjc_preprocessor_flags, [])
    kwargs["exported_preprocessor_flags"] = kwargs.get("exported_preprocessor_flags", []) + _platform_value(
        fbandroid_exported_preprocessor_flags, fbobjc_exported_preprocessor_flags, []
    )
    kwargs["deps"] = (kwargs.pop("deps", None) or []) + _platform_value(_cxx_deps(fbandroid_deps), [], _cxx_deps(cxx_deps))
    kwargs["exported_deps"] = (kwargs.pop("exported_deps", None) or []) + _platform_value(_cxx_deps(fbandroid_exported_deps), [], _cxx_deps(cxx_exported_deps))
    kwargs["target_compatible_with"] = (kwargs.pop("target_compatible_with", None) or []) + xplat_target_compatible_with(platforms, apple_sdks)
    headers = kwargs.pop("headers", None)
    if exported_headers != None:
        headers = exported_headers
    elif headers == None:
        headers = raw_headers
    cpp_library(name = name, headers = headers, **kwargs)

def _cxx_deps(deps):
    return selects.apply(deps or [], lambda values: ["fbsource" + dep if dep.startswith("//") else dep for dep in values])

def _platform_value(android, apple, cxx):
    return select({
        "DEFAULT": cxx or [],
        "ovr_config//os:android": android or [],
        "ovr_config//os:appletvos": apple or [],
        "ovr_config//os:iphoneos": apple or [],
        "ovr_config//os:linux": cxx or [],
        "ovr_config//os:macos": apple or [],
        "ovr_config//os:watchos": apple or [],
        "ovr_config//os:windows": [],
    })
