# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@fbsource//tools/build_defs:rust_binary.bzl", "rust_binary")

def buck_rust_binary(**kwargs):
    kwargs.setdefault("link_style", "static")

    # Link group is currently used automatically for rust in dev mode. Unfortunately, it builds
    # a binary that is not relocatable and it checks for dev mode by reading the build mode buckconfig.
    # If we don't disable link groups, we will also end up building a non-relocatable binary when
    # using opt modifier because opt modifier does not change build mode buckconfig. Work around this
    # by disabling link groups for now.
    # TODO(scottcao): Delete this line once link group macros are properly selectified.
    kwargs["link_group_map"] = []

    # JEMalloc is not (yet!) the default on MacOS so add the allocator
    # explicitly on all platforms here. Intel Mac (x86_64) jemalloc headers
    # are no longer maintained (platform is EOL); fall back to the system
    # allocator there.
    kwargs.setdefault(
        "allocator",
        select({
            "DEFAULT": "jemalloc",
            "ovr_config//os:macos-x86_64": "malloc",
        }),
    )
    rust_binary(**kwargs)
