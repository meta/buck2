# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@toolchains//:cpp_toolchain.bzl", "CxxCompilerInfo")

CxxLibraryInfo = provider(fields = ["headers", "objects", "include_folders"])

def _cpp_binary_impl(ctx: AnalysisContext) -> list[Provider]:
    sources = ctx.attrs.srcs
    extension = ".exe" if host_info().os.is_windows else ""
    out = ctx.actions.declare_output("main" + extension, has_content_based_path = False)

    cmd = cmd_args(
        [ctx.attrs.toolchain[CxxCompilerInfo].compiler_path, "-o", out.as_output()] + sources,
        hidden = ctx.attrs.headers,
    )

    ctx.actions.run(cmd, category = "compile")

    return [
        DefaultInfo(default_output = out),
        RunInfo(args = cmd_args(out)),
    ]

cpp_binary = rule(
    impl = _cpp_binary_impl,
    attrs = {
        "deps": attrs.list(attrs.dep()),
        "headers": attrs.list(attrs.source()),
        "srcs": attrs.list(attrs.source()),
        "toolchain": attrs.toolchain_dep(),
    },
)

def _cpp_library_impl(ctx: AnalysisContext) -> list[Provider]:
    sources = ctx.attrs.srcs
    headers = ctx.attrs.headers
    extension = ".dll" if host_info().os.is_windows else ".so"
    out = ctx.actions.declare_output("lib" + extension, has_content_based_path = False)

    cmd = cmd_args(
        [ctx.attrs.toolchain[CxxCompilerInfo].compiler_path, "-shared", "-undefined", "dynamic_lookup", "-o", out.as_output()] + sources,
        hidden = ctx.attrs.headers,
    )

    ctx.actions.run(cmd, category = "compile")

    return [DefaultInfo(default_output = out), CxxLibraryInfo(objects = [out], headers = headers)]

cpp_library = rule(
    impl = _cpp_library_impl,
    attrs = {
        "deps": attrs.list(attrs.dep()),
        "headers": attrs.list(attrs.source()),
        "srcs": attrs.list(attrs.source()),
        "toolchain": attrs.toolchain_dep(),
    },
)
