# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:genrule.bzl", "genrule_attributes")
load("@prelude//decls:common.bzl", "buck")
load("@prelude//decls:toolchains_common.bzl", "toolchains_common")

def _select_platform():
    return select({
        "DEFAULT": select({
            "DEFAULT": "android",
            "config//os/constraints:appletvos": "ios",
            "config//os/constraints:iphoneos": "ios",
            "config//os/constraints:macos": "macos",
            "config//os/constraints:windows": "windows",
        }),
        "config//react-native:macos": "macos",
        "fbsource//tools/build_defs/js/constraints/metro_js_platform_override:metro_js_platform_override[android]": "android",
        "fbsource//tools/build_defs/js/constraints/metro_js_platform_override:metro_js_platform_override[ios]": "ios",
        "fbsource//tools/build_defs/js/constraints/metro_js_platform_override:metro_js_platform_override[macos]": "macos",
        "fbsource//tools/build_defs/js/constraints/metro_js_platform_override:metro_js_platform_override[vr]": "vr",
        "fbsource//tools/build_defs/js/constraints/metro_js_platform_override:metro_js_platform_override[windows]": "windows",
    })

def _is_release():
    return select({
        "DEFAULT": select({
            "DEFAULT": select({
                "DEFAULT": False,
                "config//build_mode:optimization[opt]": True,
            }),
            "config//build_mode/constraints:build_mode[release]": True,
        }),
        "config//runtime:fbcode": select({
            "DEFAULT": False,
            "config//build_mode/constraints:opt": True,
        }),
    })

def _select_asset_dest_path_resolver():
    return select({
        "DEFAULT": None,
        "fbsource//tools/build_defs/js/constraints/asset_dest_path_resolver:asset_dest_path_resolver[android]": "android",
        "fbsource//tools/build_defs/js/constraints/asset_dest_path_resolver:asset_dest_path_resolver[generic]": "generic",
    })

def js_bundle_attrs() -> dict[str, Attr]:
    return (
        {
            "android_package": attrs.option(attrs.string(), default = None),
            "bundle_name": attrs.option(attrs.string(), default = None),
            "bundle_name_for_flavor": attrs.list(attrs.tuple(attrs.string(), attrs.string()), default = []),
            "deps": attrs.list(attrs.dep(), default = []),
            "entry": attrs.one_of(attrs.set(attrs.string(), sorted = False), attrs.string()),
            "extra_json": attrs.option(attrs.arg(), default = None),
            "fallback_transform_profile": attrs.option(attrs.string(), default = None),
            "worker": attrs.exec_dep(),
            "_android_toolchain": toolchains_common.android(),
            "_is_release": attrs.bool(
                default = _is_release(),
            ),
            "_platform": attrs.string(
                default = _select_platform(),
            ),
        }
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
    )

def js_bundle_genrule_attrs() -> dict[str, Attr]:
    return (
        {
            "bash": attrs.option(attrs.arg(), default = None),
            "bundle_name": attrs.option(attrs.string(), default = None),
            "bundle_name_for_flavor": attrs.list(attrs.tuple(attrs.string(), attrs.string()), default = []),
            "cacheable": attrs.option(attrs.bool(), default = None),
            "cmd": attrs.option(attrs.arg(), default = None),
            "cmd_exe": attrs.option(attrs.arg(), default = None),
            "enable_sandbox": attrs.option(attrs.bool(), default = None),
            "environment_expansion_separator": attrs.option(attrs.string(), default = None),
            "has_content_based_path": attrs.bool(
                default = select({
                    "DEFAULT": False,
                    "config//features/apple:content_based_path_hashing_enabled": True,
                }),
            ),
            "js_bundle": attrs.dep(),
            "remote": attrs.option(attrs.bool(), default = None),
            "rewrite_deps_file": attrs.bool(default = False),
            "rewrite_misc": attrs.bool(default = False),
            "rewrite_sourcemap": attrs.bool(default = False),
            "skip_resources": attrs.bool(default = False),
            "srcs": attrs.named_set(attrs.source(), sorted = False, default = []),
            "type": attrs.string(
                default = "js_bundle_genrule",
            ),
            "weight": attrs.option(attrs.int(), default = None),
            "_exec_os_type": buck.exec_os_type_arg(),
            "_is_release": attrs.bool(
                default = _is_release(),
            ),
            "_platform": attrs.string(
                default = _select_platform(),
            ),
        }
        | genrule_attributes()
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
    )

def js_library_attrs() -> dict[str, Attr]:
    return (
        {
            "asset_extensions": attrs.option(attrs.set(attrs.string(), sorted = False), default = None),
            "asset_platforms": attrs.option(attrs.set(attrs.string(), sorted = False), default = None),
            "base_path": attrs.option(attrs.string(), default = None),
            "deps": attrs.list(attrs.dep(), default = []),
            "deps_query": attrs.option(attrs.query(), default = None),
            "extra_babel_plugins": attrs.list(
                attrs.one_of(attrs.dep(), attrs.tuple(attrs.dep(), attrs.arg(default = "{}"))),
                default = [],
            ),
            "extra_json": attrs.option(attrs.arg(), default = None),
            "srcs": attrs.list(attrs.one_of(attrs.source(), attrs.tuple(attrs.source(), attrs.string())), default = []),
            "worker": attrs.exec_dep(),
            "_asset_dest_path_resolver": attrs.option(
                attrs.string(),
                default = _select_asset_dest_path_resolver(),
            ),
            "_is_release": attrs.bool(
                default = _is_release(),
            ),
            "_platform": attrs.string(
                default = _select_platform(),
            ),
        }
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
    )
