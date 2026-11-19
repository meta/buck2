# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//apple:apple_universal_binaries.bzl", "get_universal_binary_name", "lipo_binaries")
load("@prelude//cxx:cxx_toolchain_types.bzl", "CxxToolchainInfo")

def cxx_universal_executable_impl(ctx: AnalysisContext) -> list[Provider]:
    lipo = ctx.attrs._cxx_toolchain[CxxToolchainInfo].lipo
    if not lipo:
        fail("`cxx_toolchain()` target does not have a valid `lipo` tool: {}".format(ctx.attrs._cxx_toolchain.label))

    universal_binary = lipo_binaries(
        ctx = ctx,
        binary_deps = ctx.attrs.executable,
        binary_name = get_universal_binary_name(ctx),
        lipo = lipo,
    )

    sub_targets = {arch: [DefaultInfo(default_output = binary[DefaultInfo].default_outputs[0])] for arch, binary in ctx.attrs.executable.items()}

    return [
        DefaultInfo(
            default_output = universal_binary,
            sub_targets = sub_targets,
        ),
        RunInfo(args = cmd_args(universal_binary)),
    ]
