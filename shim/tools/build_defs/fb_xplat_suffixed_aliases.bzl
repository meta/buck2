# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@fbsource//tools/build_defs:fb_native_wrapper.bzl", "fb_native")

def create_forwarding_aliases(name, actual_name, visibility = None, **kwargs):
    _unused = kwargs  # @unused
    fb_native.alias(
        name = name,
        actual = actual_name,
        visibility = visibility or ["PUBLIC"],
    )
