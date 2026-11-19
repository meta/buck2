# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@fbsource//tools/build_defs:platform_defs.bzl", "CXX", "FBCODE", "xplat_target_compatible_with")
load("@shim//build_defs:cpp_binary.bzl", "cpp_binary")

def fb_dirsync_cpp_binary(name, platforms = (CXX, FBCODE), apple_sdks = (), xplat_impl = None, **kwargs):
    _unused = xplat_impl  # @unused
    kwargs["target_compatible_with"] = (kwargs.pop("target_compatible_with", None) or []) + xplat_target_compatible_with(platforms, apple_sdks)
    cpp_binary(name = name, **kwargs)
