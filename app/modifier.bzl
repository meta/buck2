# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@fbcode//buck2/cfg/experimental:modifiers.bzl", "modifiers")

def buck2_modifiers():
    # **WARNING**: This is not vetted for correctness and should only be used in fbcode/buck2.
    # A somewhat hacked together list of modifiers to enable mode-free builds and opt-by-default-cxx builds.
    # This currently only works for linux and mac but not for cross-building (ex. build mac from linux)
    # - Mode-free builds: Users can use `-m opt` or `--modifier opt` instead of `@fbcode//mode/opt` to trigger builds
    #   E2e tests build with opt buck2 by default.
    # - Opt-by-default cxx: Dev mode builds of buck2 comes with optimized, sanitizer-free cxx deps by default, making
    #   dev mode buck2 significantly more usable
    #
    # This is a demo that all of this could be done with modifiers. Most of these modifiers should be set on higher-level
    # PACKAGE files like fbcode/PACKAGE or fbsource/PACKAGE.
    #
    # Known problems:
    # - We have to explicitly disable link groups at the moment because link group macros are supposed to only be turned
    #   on for dev mode and it checks for this by reading the dev mode buckconfig, but the opt modifier also uses dev
    #   mode buckconfig, and as a result the presence of link group map breaks our opt modifier build.

    return [
        "ovr_config//build_mode/constraints:opt",
        modifiers.conditional({
            "DEFAULT": None,
            "ovr_config//os:linux": modifiers.conditional({
                "DEFAULT": modifiers.conditional({
                    "DEFAULT": "ovr_config//build_mode/constraints:default_link_style[static]",
                    "ovr_config//build_mode:dev": "ovr_config//build_mode/constraints:default_link_style[shared]",
                }),
                "ovr_config//build_mode:sanitizer_type[asan]": "ovr_config//build_mode/constraints:default_link_style[static_pic]",
            }),
        }),
        "ovr_config//build_mode:sanitizer_type[no-san]",
        modifiers.conditional({
            "DEFAULT": "ovr_config//build_mode/constraints:fbcode-build-info-mode[stable]",
            "ovr_config//build_mode:opt": "ovr_config//build_mode/constraints:fbcode-build-info-mode[full]",
        }),
        modifiers.conditional({
            "DEFAULT": "ovr_config//build_mode/constraints:python-default-package-style[inplace]",
            "ovr_config//build_mode:opt": "ovr_config//build_mode/constraints:python-default-package-style[standalone]",
        }),
        modifiers.conditional({
            "DEFAULT": None,
            "ovr_config//os:macos": "ovr_config//build_mode/constraints:fbcode-build-info-ldflags[accepted]",
        }),
        modifiers.conditional({
            "DEFAULT": None,
            "ovr_config//os:macos": "ovr_config//build_mode/constraints:fbcode-custom-allocators[enabled]",
        }),
        modifiers.conditional({
            "DEFAULT": None,
            "ovr_config//runtime:fbcode": modifiers.conditional({
                "ovr_config//cpu:arm64": "ovr_config//runtime/constraints:platform010-aarch64",
                "ovr_config//cpu:x86_64": modifiers.conditional({
                    "DEFAULT": "ovr_config//runtime/constraints:platform010",
                    "ovr_config//cpp/constraints:libc++": "ovr_config//runtime/constraints:platform010-libcxx",
                }),
            }),
        }),
        modifiers.conditional({
            "DEFAULT": None,
            "ovr_config//runtime:fbcode": modifiers.conditional({
                "ovr_config//cpu:arm64": "ovr_config//cpu/constraints:nosve2",
                "ovr_config//cpu:x86_64": "ovr_config//cpu/constraints:sve2",
            }),
        }),
        modifiers.conditional({
            "DEFAULT": None,
            "ovr_config//os:linux": modifiers.conditional({
                "DEFAULT": None,
                "ovr_config//cpu:arm64": "ovr_config//cpu/constraints:armv9.0a",
            }),
        }),
    ]
