# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//utils:buckconfig.bzl", "read_bool")
load("@shim//tools/build_defs:platform_defs.bzl", "APPLETVOS", "IOS", "MACOSX")

DEFAULT_APPLE_SDKS = (IOS, APPLETVOS, MACOSX)

FBANDROID_CPPFLAGS = []

WINDOWS_CLANG_CXX_FLAGS = []

def should_enable_gflags():
    return read_bool("folly", "have_libgflags_override", False)
