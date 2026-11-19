# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//cxx:cxx_context.bzl", "get_cxx_toolchain_info")
load("@prelude//utils:expect.bzl", "expect")

def apple_strip_args(ctx: AnalysisContext) -> cmd_args:
    strip_flags_info = get_cxx_toolchain_info(ctx).strip_flags_info
    if ctx.attrs.strip_level == "non_global":
        flags = strip_flags_info.strip_non_global_flags
    elif ctx.attrs.strip_level == "all":
        flags = strip_flags_info.strip_all_flags
    elif ctx.attrs.strip_level == "debug":
        flags = strip_flags_info.strip_debug_flags
    else:
        fail("Unexpected strip level: " + ctx.attrs.strip_level)

    expect(flags != None, "Strip flags are missing on toolchain")
    return cmd_args(flags)
