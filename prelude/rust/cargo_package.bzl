# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""
This file exports utilities for building third-party code with reindeer.
These are not used anywhere else in prelude and are not exported as prelude globals.

These should not be used for first-party code where lints are desired.
"""

load("@prelude//:prelude.bzl", "native")
load("@prelude//utils:selects.bzl", "selects")
load("@prelude//utils:type_defs.bzl", "is_dict", "is_list")

def set_reindeer_platforms(platforms) -> None:
    """
    Call from a PACKAGE or BUCK_TREE file to make the macros in this file
    recognize your own non-default platforms.

        load("@prelude//rust:cargo_package.bzl", "DEFAULT_REINDEER_PLATFORMS", "set_reindeer_platforms")

        set_reindeer_platforms(select({
            "DEFAULT": DEFAULT_REINDEER_PLATFORMS,  # (optional)
            "ovr_config//cpu:arm32-embedded-fpu": "thumbv7em-none-eabihf",
        }))
    """

    native.write_package_value(
        "rust.reindeer_platforms",
        platforms,
        overwrite = True,
    )
    native.write_package_value(
        "rust.reindeer_platform_names",
        _reindeer_platform_names(platforms),
        overwrite = True,
    )

def get_reindeer_platforms():
    platforms = native.read_package_value("rust.reindeer_platforms")
    if platforms != None:
        return platforms
    return DEFAULT_REINDEER_PLATFORMS

def get_reindeer_platform_names() -> set[str]:
    platform_names = native.read_package_value("rust.reindeer_platform_names")
    if platform_names != None:
        return platform_names
    return _DEFAULT_REINDEER_PLATFORM_NAMES

DEFAULT_REINDEER_PLATFORMS = select({
    "DEFAULT": None,
    "prelude//os:linux": select({
        "DEFAULT": None,
        "prelude//cpu:arm64": "linux-arm64",
        "prelude//cpu:riscv64": "linux-riscv64",
        "prelude//cpu:x86_64": "linux-x86_64",
    }),
    "prelude//os:macos": select({
        "DEFAULT": None,
        "prelude//cpu:arm64": "macos-arm64",
        "prelude//cpu:x86_64": "macos-x86_64",
    }),
    "prelude//os:none": select({
        "DEFAULT": None,
        "prelude//cpu:wasm32": "wasm32",
    }),
    "prelude//os:wasi": select({
        "DEFAULT": None,
        "prelude//cpu:wasm32": "wasi",
    }),
    "prelude//os:windows": select({
        "DEFAULT": "windows-msvc",
        "prelude//abi:gnu": "windows-gnu",
        "prelude//abi:msvc": "windows-msvc",
    }),
})

def _reindeer_platform_names(platform_select) -> set[str]:
    names = set()
    selects.apply(
        platform_select,
        lambda plat: names.add(plat) if plat != None else None,
    )
    return names

_DEFAULT_REINDEER_PLATFORM_NAMES = _reindeer_platform_names(DEFAULT_REINDEER_PLATFORMS)

def apply_platform_attrs(platform_attrs, universal_attrs, platform_select = None):
    if platform_select == None:
        platform_select = get_reindeer_platforms()

    platform_attr_defaults = {}
    for attrs in platform_attrs.values():
        for attr, value in attrs.items():
            if native.select_test(value, is_list):
                platform_attr_defaults[attr] = []
            elif native.select_test(value, is_dict):
                platform_attr_defaults[attr] = {}
            else:
                platform_attr_defaults[attr] = None

    combined_attrs = dict(universal_attrs)
    for attr, default_value in platform_attr_defaults.items():
        conditional_value = selects.apply(
            platform_select,
            lambda platform: platform_attrs.get(platform, {}).get(attr, default_value),
        )
        if attr in combined_attrs:
            combined_attrs[attr] = combined_attrs[attr] + conditional_value
        else:
            combined_attrs[attr] = conditional_value

    return combined_attrs

# Build scripts need to evaluate `platform` according to the target platform of
# the corresponding build-script-run target, not the current platform they are
# configured for (which is the execution platform of the build-script-run
# target).
def apply_platform_attrs_for_buildscript_build(platform_attrs, universal_attrs):
    return apply_platform_attrs(
        platform_attrs,
        universal_attrs,
        select(
            {
                "DEFAULT": get_reindeer_platforms(),
            }
            | {"prelude//rust/buildscript:buildscript_for_platform[{}]".format(i): plat for i, plat in enumerate(get_reindeer_platform_names())}
        ),
    )

def _cargo_rust_binary(name, crate = None, platform = {}, **kwargs):
    """
    Build a third-party rust binary for reindeer, suppressing lints.
    """

    if crate == "build_script_build":
        kwargs = apply_platform_attrs_for_buildscript_build(platform, kwargs)
    else:
        kwargs = apply_platform_attrs(platform, kwargs)

    rustc_flags = kwargs.get("rustc_flags", [])
    kwargs["rustc_flags"] = ["--cap-lints=allow"] + rustc_flags

    native.rust_binary(name = name, crate = crate, **kwargs)

def _cargo_rust_library(name, platform = {}, **kwargs):
    """
    Build a third-party rust library for reindeer, suppressing lints.
    """

    kwargs = apply_platform_attrs(platform, kwargs)

    rustc_flags = kwargs.get("rustc_flags", [])
    kwargs["rustc_flags"] = ["--cap-lints=allow"] + rustc_flags

    kwargs.setdefault("doctests", False)

    # Support for reindeer's `python_ext` fixup is not implemented yet.
    kwargs.pop("dlopen_enable", None)
    kwargs.pop("linkable_alias", None)

    native.rust_library(name = name, **kwargs)

def alias(name, actual, platforms = None, visibility = None):
    if platforms == None:
        target_compatible_with = selects.apply(
            get_reindeer_platforms(),
            lambda plat: ["prelude//:none"] if plat == None else [],
        )
    else:
        target_compatible_with = selects.apply(
            get_reindeer_platforms(),
            lambda plat: [] if plat in platforms else ["prelude//:none"],
        )

    native.alias(
        name = name,
        actual = actual,
        target_compatible_with = target_compatible_with,
        visibility = visibility,
    )

cargo = struct(
    rust_binary = _cargo_rust_binary,
    rust_library = _cargo_rust_library,
)
